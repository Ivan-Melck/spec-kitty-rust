//! Org-charter policy import: `required_<kind>` floors, provenance and the
//! org-tier mission overrides.
//!
//! Mirrors `src/charter/activation/org_charter.py`,
//! `org_pack_discovery.py`, `org_expected_artifacts.py` and
//! `org_requirement_kinds.py`.
//!
//! Two capabilities live here, and they are deliberately distinct:
//!
//! * the **raw** reader/walk in [`source`], which owns `org-charter.yaml` reads
//!   and drives every Source floor/selection/namespace consumer
//!   (`_iter_org_charter_docs`, `_required_values`, `read_org_required_ids`,
//!   `_read_org_required_selections`, `read_org_skill_namespace`);
//! * the **typed** whole-policy capability ([`load_org_charter_policy`]), which
//!   validates a complete `OrgCharterPolicy` for the callers that need one
//!   (the `charter context --json` org block). It is never the floor's input.
//!
//! The floor is the union of every declared org pack's `required_<kind>` list
//! over the pack's `extends:` chain, first-seen order, normalized to activation
//! stems. It is the `R` in `D* = D − R` (see
//! [`crate::ledger::effective_deny`]), so a stale project deny can never remove
//! an org-required artifact from a cascade.
//!
//! **The document is read exactly.** The reference loads `org-charter.yaml`
//! with `ruamel.yaml`'s safe reader, so a requirement token authored as
//! `"author\ud800"` is a Python `str` holding code point `U+D800`, and its
//! `IdentityIndex` — keyed on exact Python strings — resolves that token to the
//! artifact whose declared id is the identical sequence. This module therefore
//! reads the charter through the *same* canonical transport and construction
//! phase as the profile loader ([`crate::profile_text`] +
//! [`crate::profile_construct`]) and carries every requirement token as a
//! [`sk_kernel::PythonString`]. There is no second YAML reader, no shadow codec
//! and no lossy token projection on this path.
//!
//! A missing or unreadable *declared* pack is a refusal in strict mode; the
//! lenient mode is the best-effort diagnostics path used while rendering.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_yaml_ng::{Mapping, Value};
use sk_charter::ArtifactKind;
use sk_kernel::PythonString;
use sk_kernel::python_json::PythonJsonValue;

use crate::error::DoctrineError;
use crate::identity::IdentityIndex;

mod chain;
mod fold;
mod source;

pub use chain::OrgPackContext;
pub use fold::{
    OrgCharterPolicyFold, fold_org_charter_policies, fold_org_charter_policies_with_context,
};
pub use source::OrgCharterRawDocument;

/// The org-charter policy file name inside a pack root.
pub const ORG_CHARTER: &str = "org-charter.yaml";
/// Schema version new and scaffolded files declare.
pub const ORG_CHARTER_SCHEMA_VERSION: i64 = 2;

/// The built-in pack directory name that a packs-root value may name instead
/// of its parent.
const BUILT_IN_DIR: &str = "built-in";

/// Normalize a packs-root value to the documented pack **parent**.
///
/// `SPEC_KITTY_PACKS_ROOT` is documented as the parent of `built-in`
/// (`<...>/packs`), but the CLI adapter also accepts the built-in leaf
/// (`<...>/packs/built-in`) and normalizes it to the parent. This is the same
/// normalization, shared by every root read in this crate so a leaf spelling
/// cannot make one seam (the org required floor) disagree with another (the
/// CLI's pack/template reads).
pub fn normalize_packs_root(value: impl AsRef<Path>) -> PathBuf {
    let value = value.as_ref();
    if value.file_name().and_then(std::ffi::OsStr::to_str) == Some(BUILT_IN_DIR)
        && let Some(parent) = value.parent()
    {
        return parent.to_path_buf();
    }
    value.to_path_buf()
}

/// The `SPEC_KITTY_PACKS_ROOT` override, normalized to the pack parent.
pub fn packs_root_override() -> Option<PathBuf> {
    std::env::var_os("SPEC_KITTY_PACKS_ROOT")
        .map(PathBuf::from)
        .map(normalize_packs_root)
}

/// Build the built-in-tier search with the documented, normalized packs root.
///
/// Keeping this in one place is what makes the floor, the context roots and
/// the identity index agree on the leaf/parent spelling; a raw
/// `BuiltInSearch::from_env` read would fail its search for a leaf value.
pub(crate) fn built_in_search(_project_root: &Path) -> sk_charter::BuiltInSearch {
    // Like Source's package-relative resource anchor, not the caller's
    // (potentially unrelated) project directory.
    let mut search = sk_charter::BuiltInSearch::packaged();
    if let Some(root) = packs_root_override() {
        search = search.with_packs_root_env(root);
    }
    search
}

const REQUIRED_PREFIX: &str = "required_";

/// Kinds whose `required_<plural>` list unions into a `selected_<kind>` field.
const REQUIRED_KINDS: [ArtifactKind; 8] = [
    ArtifactKind::Directive,
    ArtifactKind::Tactic,
    ArtifactKind::Paradigm,
    ArtifactKind::Styleguide,
    ArtifactKind::Toolguide,
    ArtifactKind::Procedure,
    ArtifactKind::AgentProfile,
    ArtifactKind::MissionStepContract,
];

/// Every kind an org charter may require (the overlayable set plus glossary
/// packs, assets and skills, which have no `selected_` overlay).
pub fn org_requirable_kinds() -> Vec<ArtifactKind> {
    let mut kinds = REQUIRED_KINDS.to_vec();
    for extra in [
        ArtifactKind::GlossaryPack,
        ArtifactKind::Asset,
        ArtifactKind::Skill,
    ] {
        if !kinds.contains(&extra) {
            kinds.push(extra);
        }
    }
    kinds
}

/// Kinds whose `required_<plural>` list is a selection overlay.
pub fn selection_overlayable_kinds() -> &'static [ArtifactKind] {
    &REQUIRED_KINDS
}

/// One pack that requires an artifact.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct RequiringPack {
    /// The chain member whose own `org-charter.yaml` lists the id.
    pub pack: String,
    /// The declared pack whose `extends:` chain inherits it (`None` when the
    /// member is itself the declared pack).
    pub via: Option<String>,
}

impl RequiringPack {
    /// `'base'` or `'base' (via 'overlay')`.
    pub fn describe(&self) -> String {
        match &self.via {
            None => format!("'{}'", self.pack),
            Some(via) => format!("'{}' (via '{}')", self.pack, via),
        }
    }
}

/// A single `governance-policies` entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GovernancePolicy {
    /// The policy field.
    pub field: PythonString,
    /// The policy value, exactly as authored (`str | bool`).
    pub value: AuthoredScalar,
    /// The enforcement level (`advisory` today).
    pub enforcement: PythonString,
}

/// One `activations` registry entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationEntry {
    /// The activation context, exactly (`activation_context`): an ordered
    /// `str -> str` mapping, as the reference types it (`dict[str, str]`).
    /// Insertion order is preserved; the dedup identity is keyed on the pairs
    /// **sorted by key**, exactly like the reference's `_activation_identity_key`.
    pub context: Vec<(PythonString, PythonString)>,
    /// The declaring charter pack id.
    pub charter_pack_id: PythonString,
    /// The artifact id.
    pub artifact_id: PythonString,
    /// The artifact kind (canonical plural, as the reference normalises it), or
    /// `None` when the entry leaves it out (the reference's `str | None`).
    pub artifact_kind: Option<PythonString>,
}

impl ActivationEntry {
    /// Whether two entries share the reference's 4-tuple dedup identity
    /// `(activation_context, charter_pack_id, artifact_id, artifact_kind)`,
    /// with the context compared on its pairs sorted by key.
    fn same_identity(&self, other: &Self) -> bool {
        self.artifact_id == other.artifact_id
            && self.charter_pack_id == other.charter_pack_id
            && self.artifact_kind == other.artifact_kind
            && sorted_pairs(&self.context) == sorted_pairs(&other.context)
    }
}

fn sorted_pairs(context: &[(PythonString, PythonString)]) -> Vec<(PythonString, PythonString)> {
    let mut pairs = context.to_vec();
    pairs.sort_by(|left, right| left.0.cmp(&right.0));
    pairs
}

/// The authored `interview_defaults` mapping of one `org-charter.yaml`.
///
/// A Python `dict` in the reference, so this is an **insertion-ordered** map:
/// iteration is the document's key order, and re-inserting an existing key
/// replaces its value **in place** (`dict.update` / a duplicate YAML key).
/// Sorting the keys (the previous `BTreeMap`) silently changed the order in which
/// the charter interview consumes the org defaults; `interview_defaults` is also
/// what the pack-context fold merges per key, so a sorted container reorders the
/// merged result too.
///
/// Lookup is a linear scan: these mappings hold a handful of interview keys, and
/// keeping one container avoids a second index that could disagree with the
/// authored order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuthoredDefaults {
    entries: Vec<(PythonString, AuthoredScalar)>,
}

impl AuthoredDefaults {
    /// The empty mapping.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert `key`, replacing an existing key's value at its original position.
    ///
    /// This is Python `dict` assignment: the first declaration owns the slot, the
    /// last declaration owns the value.
    pub fn insert(&mut self, key: PythonString, value: AuthoredScalar) {
        match self
            .entries
            .iter_mut()
            .find(|(candidate, _)| candidate == &key)
        {
            Some(slot) => slot.1 = value,
            None => self.entries.push((key, value)),
        }
    }

    /// The value of `key`, exactly as the last declaring pack/document wrote it.
    pub fn get(&self, key: &PythonString) -> Option<&AuthoredScalar> {
        self.entries
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value)
    }

    /// Whether `key` is declared.
    pub fn contains_key(&self, key: &PythonString) -> bool {
        self.get(key).is_some()
    }

    /// Every entry, in the authored order.
    pub fn iter(&self) -> std::slice::Iter<'_, (PythonString, AuthoredScalar)> {
        self.entries.iter()
    }

    /// Every authored key, in order.
    pub fn keys(&self) -> impl Iterator<Item = &PythonString> {
        self.entries.iter().map(|(key, _)| key)
    }

    /// The number of declared keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is declared.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl FromIterator<(PythonString, AuthoredScalar)> for AuthoredDefaults {
    /// Collect in iterator order through [`AuthoredDefaults::insert`], so a
    /// duplicate key keeps its first slot.
    fn from_iter<I: IntoIterator<Item = (PythonString, AuthoredScalar)>>(iter: I) -> Self {
        let mut defaults = Self::new();
        for (key, value) in iter {
            defaults.insert(key, value);
        }
        defaults
    }
}

impl<'a> IntoIterator for &'a AuthoredDefaults {
    type Item = &'a (PythonString, AuthoredScalar);
    type IntoIter = std::slice::Iter<'a, (PythonString, AuthoredScalar)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

impl IntoIterator for AuthoredDefaults {
    type Item = (PythonString, AuthoredScalar);
    type IntoIter = std::vec::IntoIter<(PythonString, AuthoredScalar)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

/// One org pack's parsed `org-charter.yaml`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrgCharterPolicy {
    /// The file it was read from.
    pub path: PathBuf,
    /// The pack root.
    pub pack_root: PathBuf,
    /// Declared schema version, exactly (a Python `int`: arbitrary width and
    /// every spelling CPython's `int(str)` accepts).
    pub schema_version: PythonInt,
    /// The `extends:` base pack name.
    pub extends: Option<String>,
    /// The org name, exactly.
    pub org_name: Option<PythonString>,
    /// Interview defaults, exact keys and exact `str | bool` values, in the
    /// authored mapping order.
    ///
    /// The reference types this field as a Python `dict`, so iteration order is
    /// the YAML document's own key order and a later override keeps the FIRST
    /// declaration's position (a sorted container would silently reorder the
    /// interview defaults the operator sees). [`AuthoredDefaults`] is that order
    /// authority; it is not a lookup-optimised map.
    pub interview_defaults: AuthoredDefaults,
    /// `required_<plural>` lists, verbatim order, exactly.
    pub required: BTreeMap<ArtifactKind, Vec<PythonString>>,
    /// The skill namespace prefix, exactly.
    pub skill_namespace: Option<PythonString>,
    /// Governance policies.
    pub governance_policies: Vec<GovernancePolicy>,
    /// Activation registry entries.
    pub activations: Vec<ActivationEntry>,
}

impl OrgCharterPolicy {
    /// The `required_<plural>` list of `kind`, exactly.
    pub fn required_for(&self, kind: ArtifactKind) -> &[PythonString] {
        self.required.get(&kind).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The `required_<plural>` list of `kind` as the **scalar capability view**.
    ///
    /// Tokens with no scalar projection (a lone surrogate) are omitted here;
    /// [`OrgCharterPolicy::required_for`] carries them. The floor and identity
    /// paths never use this view.
    pub fn required_scalar(&self, kind: ArtifactKind) -> Vec<String> {
        self.required_for(kind)
            .iter()
            .filter_map(|token| token.to_scalar_string().ok())
            .collect()
    }
}

/// The unioned required floor across every configured org pack.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RequiredFloor {
    /// Normalized activation stems per kind (first-seen order). A stem is a
    /// file name, so it is always a scalar; the exact authored token lives in
    /// [`RequiredFloor::unresolved_exact`] when it resolves to no layer.
    pub tokens: BTreeMap<ArtifactKind, Vec<String>>,
    /// Normalized activation stem -> the packs that require it.
    pub provenance: BTreeMap<(ArtifactKind, String), Vec<RequiringPack>>,
    /// Ids that resolve in no readable layer (kept as written).
    pub unresolved: Vec<String>,
    /// Ids that resolve in no readable layer, exactly as authored (a token may
    /// be a Python `str` with a lone surrogate; it is still a real token).
    pub unresolved_exact: Vec<PythonString>,
}

impl RequiredFloor {
    /// The floor for one kind in the shape [`crate::plan::ActivationInputs`] and
    /// [`crate::activation::ActivationState`] consume.
    pub fn into_map(self) -> BTreeMap<ArtifactKind, Vec<String>> {
        self.tokens
    }
}

/// One constructed `org-charter.yaml` document plus its exact-scalar table.
///
/// The tuple the raw owner's reader hands to the **typed** whole-policy loader
/// ([`source`]'s `read_mapping`): the constructed mapping, and the transport marker
/// table that carries lossy (surrogate-bearing) strings exactly. A named alias so
/// the phase's type is documented once and every projection borrows the same one.
type OrgCharterDocument = (Mapping, BTreeMap<String, PythonString>);

fn describe(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Sequence(_) => "a list",
        Value::Mapping(_) => "a mapping",
        Value::Tagged(_) => "a tagged value",
    }
}

fn required_key(kind: ArtifactKind) -> String {
    format!("{REQUIRED_PREFIX}{}", kind.plural())
}

/// The exact value of a constructed Python `str`, through the transport's table.
///
/// The reference model types these fields `str` (`extends`, `org_name`,
/// `skill_namespace`, every `required_<kind>` member, `interview_defaults`
/// keys, `governance_policies` ids and `activations` ids), and pydantic v2 does
/// **not** coerce numbers to `str`. A non-string is therefore a schema failure —
/// never a silently stringified number, which would also change the floor's
/// tokens.
fn exact_python_str(
    value: &Value,
    markers: &BTreeMap<String, PythonString>,
) -> Option<PythonString> {
    match value {
        Value::String(text) => Some(match markers.get(text) {
            Some(exact) => exact.clone(),
            None => PythonString::from_scalars(text),
        }),
        _ => None,
    }
}

/// One authored `str | bool` policy value.
///
/// The reference model types `interview_defaults` values and
/// `governance_policies[*].value` as `str | bool`. Pydantic keeps an admitted
/// Python `str` (or a `bytes` value that decodes as UTF-8, its lax `str`) as
/// itself and coerces only a real `bool` (and the numbers `0`/`1`, including the
/// floats `0.0`/`1.0`) to a Python `bool` — probed against the pinned Source:
/// `true` -> `True`, `1` -> `True`, `0.0` -> `False`, `'True'` -> the **string**
/// `'True'`, `!!binary aGk=` -> the **string** `'hi'`, `2`/`2.0`/`1.5`/`null`/
/// non-UTF-8 bytes -> invalid.
///
/// A text-only Rust view loses that distinction: it would both stringify an
/// authored boolean and make `(field, True)` and `(field, 'True')` the same
/// `(field, value)` dedupe identity in the cross-pack fold. So the value type is
/// explicit and every consumer projects it through an accessor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthoredScalar {
    /// A Python `str`, exactly (it may carry a lone surrogate).
    Text(PythonString),
    /// A Python `bool`.
    Bool(bool),
}

impl AuthoredScalar {
    /// The value as authored text, when it is one.
    pub fn as_text(&self) -> Option<&PythonString> {
        match self {
            Self::Text(text) => Some(text),
            Self::Bool(_) => None,
        }
    }

    /// The value as an authored boolean, when it is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(flag) => Some(*flag),
            Self::Text(_) => None,
        }
    }

    /// Python `str(value)`: the exact text, or `True`/`False`.
    pub fn python_str(&self) -> PythonString {
        match self {
            Self::Text(text) => text.clone(),
            Self::Bool(true) => PythonString::from_scalars("True"),
            Self::Bool(false) => PythonString::from_scalars("False"),
        }
    }

    /// Python `bool(value)`: a non-empty `str`, or the boolean itself.
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Text(text) => !text.is_empty(),
            Self::Bool(flag) => *flag,
        }
    }

    /// The exact JSON payload: `true`/`false` for a boolean, the exact code
    /// points for a string. This is the reference's `model_dump()` value.
    pub fn to_json_value(&self) -> PythonJsonValue {
        match self {
            Self::Text(text) => PythonJsonValue::String(text.clone()),
            Self::Bool(flag) => PythonJsonValue::Bool(*flag),
        }
    }

    /// pydantic's `str | bool` admission for a constructed document value.
    ///
    /// An exact `str` wins (the union's smart-mode exact-type match), otherwise
    /// the value must be a Python `bool` or one of the numbers pydantic's lax
    /// `bool` accepts (`0`/`1`, `0.0`/`1.0`). A constructed big integer is never
    /// `0` or `1`, so it is invalid here exactly as it is in the reference.
    fn admit(value: &Value, markers: &BTreeMap<String, PythonString>) -> Option<Self> {
        if let Some(text) = exact_python_str(value, markers) {
            return Some(Self::Text(text));
        }
        // pydantic's lax `str` also accepts `bytes`, decoded strictly as UTF-8
        // (probed: `!!binary aGk=` is the text `'hi'`, `!!binary /w==` is invalid).
        // The union's `str` member is tried before `bool`, so UTF-8 bytes are text.
        if let Some(bytes) = sk_kernel::python_yaml::constructed_bytes(value) {
            return std::str::from_utf8(&bytes)
                .ok()
                .map(|text| Self::Text(PythonString::from_scalars(text)));
        }
        let flag = match value {
            Value::Bool(flag) => *flag,
            Value::Number(number) => {
                if let Some(int) = number.as_i64() {
                    match int {
                        0 => false,
                        1 => true,
                        _ => return None,
                    }
                } else {
                    let float = number.as_f64()?;
                    if float == 0.0 {
                        false
                    } else if float == 1.0 {
                        true
                    } else {
                        return None;
                    }
                }
            }
            _ => return None,
        };
        Some(Self::Bool(flag))
    }
}

impl serde::Serialize for AuthoredScalar {
    /// The finite `serde_json` view: a boolean stays a boolean; a string is its
    /// scalar projection, or an adapter error for a lone surrogate.
    ///
    /// [`AuthoredScalar::to_json_value`] is the exact payload authority — the CLI
    /// writes `CharterContext::to_json_exact`, not this adapter.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Bool(flag) => serializer.serialize_bool(*flag),
            Self::Text(text) => serializer.serialize_str(
                &text
                    .to_scalar_string()
                    .map_err(<S::Error as serde::ser::Error>::custom)?,
            ),
        }
    }
}

/// An exact authored text value (a `PythonString`) with a finite `serde_json`
/// view.
///
/// The exact payload is the [`PythonString`] itself; the `Serialize` impl is the
/// checked finite adapter (an error for a value with no scalar projection, i.e.
/// a lone surrogate). The context payload is built by
/// `CharterContext::to_json_exact`, which carries these exactly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExactText(PythonString);

impl ExactText {
    /// Wrap an exact authored value.
    pub fn new(text: PythonString) -> Self {
        Self(text)
    }

    /// The exact code points.
    pub fn as_python(&self) -> &PythonString {
        &self.0
    }

    /// Consume the wrapper and return the exact value.
    pub fn into_python(self) -> PythonString {
        self.0
    }
}

impl From<PythonString> for ExactText {
    fn from(text: PythonString) -> Self {
        Self(text)
    }
}

impl serde::Serialize for ExactText {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(
            &self
                .0
                .to_scalar_string()
                .map_err(<S::Error as serde::ser::Error>::custom)?,
        )
    }
}

/// An exact Python `int`, in canonical decimal text (`0`, `-12`, `123...`).
///
/// The reference types `schema_version` as Python `int` and coerces an authored
/// `str` through `int(v)`. CPython integers have **no width bound**: a 40-digit
/// version is a valid `int`, and `int()` accepts any Unicode decimal (`Nd`)
/// digit, surrounding whitespace, a sign and single underscores between digits.
/// A stored `i64` is therefore not the Source type — it refused Source-valid
/// versions — and neither is a lossy float. The value is canonicalised once
/// (sign + digits, no leading zeros, `-0` -> `0`) so equality, ordering and
/// truthiness are exact for any width.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PythonInt(String);

impl PythonInt {
    /// The exact value of a Rust `i64`.
    pub fn from_i64(value: i64) -> Self {
        Self(value.to_string())
    }

    /// The canonical decimal text (`'-'? digit+`).
    pub fn as_decimal(&self) -> &str {
        &self.0
    }

    /// Python truthiness: zero is falsy, every other integer truthy.
    pub fn is_truthy(&self) -> bool {
        self.0 != "0"
    }

    /// The exact JSON payload (`PythonJsonValue::Integer`).
    pub fn to_json_value(&self) -> PythonJsonValue {
        PythonJsonValue::Integer(self.0.clone())
    }

    /// Python `int(str)` text: surrounding `str.isspace()` characters are
    /// ignored, an optional sign is allowed, a single underscore may separate
    /// digits (`'1_0'` is `10`) and every Unicode decimal digit counts. Anything
    /// else (a trailing/leading/doubled underscore, a non-decimal numeric
    /// character) is not an integer.
    ///
    /// `digit_limit` is the interpreter's `sys.get_int_max_str_digits()` value
    /// (`0` = unlimited): CPython raises `ValueError` when the *digit count* of
    /// the argument exceeds it, and the count excludes the sign, the surrounding
    /// whitespace and the underscores (probed: `'-' + 4300 digits` is accepted,
    /// `'0'*4300 + '1'` is refused).
    pub fn parse_text(token: &PythonString, digit_limit: usize) -> Option<Self> {
        sk_kernel::python_int::parse_text(token, digit_limit)
            .ok()
            .map(Self)
    }

    /// An authored YAML integer literal, exactly.
    ///
    /// `Value::Number` covers every literal the transport resolves to a Python
    /// `int` that fits its numeric storage; a wider literal is the constructor's
    /// tagged `!!int` carrying its exact decimal digits.
    ///
    /// The three outcomes are the reference's own phases:
    ///
    /// * `Ok(Some(_))` — a Python `int` the model accepts;
    /// * `Ok(None)` — an integer literal beyond the interpreter's conversion
    ///   ceiling, which makes the reference's **YAML load** raise (the pack
    ///   document is then unreadable, exactly like any other parse failure);
    /// * `Err(())` — a value that is not a Python `int`, i.e. a schema failure.
    fn admit_yaml(value: &Value, digit_limit: usize) -> Result<Option<Self>, ()> {
        match value {
            Value::Number(number) => {
                if let Some(int) = number.as_i64() {
                    return Ok(Some(Self::from_i64(int)));
                }
                if let Some(unsigned) = number.as_u64() {
                    return Ok(Some(Self(unsigned.to_string())));
                }
                // A YAML float is a Python `float`; the reference's `int` field
                // never accepts one.
                Err(())
            }
            Value::Tagged(_) => {
                let Some(digits) = sk_kernel::python_yaml::constructed_big_integer(value) else {
                    return Err(());
                };
                let count = digits
                    .strip_prefix(['+', '-'])
                    .unwrap_or(digits)
                    .chars()
                    .count();
                if digit_limit != 0 && count > digit_limit {
                    return Ok(None);
                }
                let negative = digits.starts_with('-');
                let magnitude = digits.trim_start_matches(['+', '-']);
                Ok(Some(Self::canonical(negative, magnitude)))
            }
            _ => Err(()),
        }
    }

    /// Canonicalise `sign + digits` (leading zeros dropped, `-0` -> `0`).
    fn canonical(negative: bool, digits: &str) -> Self {
        let magnitude = digits.trim_start_matches('0');
        if magnitude.is_empty() {
            return Self("0".to_owned());
        }
        Self(if negative {
            format!("-{magnitude}")
        } else {
            magnitude.to_owned()
        })
    }
}

impl PartialOrd for PythonInt {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PythonInt {
    /// Integer ordering borrows the one lower exact numeric comparison rule.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        if self == other {
            return Ordering::Equal;
        }
        if sk_kernel::python_json::python_json_lt(&self.to_json_value(), &other.to_json_value())
            .expect("validated integer operands are comparable")
        {
            Ordering::Less
        } else {
            Ordering::Greater
        }
    }
}

/// CPython `chr(code).isspace()` for one code point.
///
/// `str.isspace()` is `White_Space` **plus** U+001C..=U+001F (the information
/// separators, which `char::is_whitespace` excludes); the pinned Kernel table is
/// the one authority.
fn python_isspace(code: u32) -> bool {
    sk_kernel::python_text::is_whitespace(code)
}

/// The interpreter's `sys.get_int_max_str_digits()` setting (`0` = unlimited).
///
/// This is the pinned Kernel's startup-setting authority; an unstartable setting
/// falls back to the documented default (the reference interpreter would not
/// have started at all, so no pack phase is reachable).
fn integer_digit_limit() -> usize {
    sk_kernel::python_json::environment_integer_digits()
        .unwrap_or(sk_kernel::python_json::SOURCE_DEFAULT_INTEGER_DIGITS)
}

/// Python's `str.strip()` whitespace set.
///
/// `str.isspace()` is `White_Space` **plus** U+001C..=U+001F (the information
/// separators, which `char::is_whitespace` excludes), so a requirement token
/// authored with a trailing file/group/record/unit separator strips exactly as
/// the reference strips it.
fn strip_token(token: &PythonString) -> PythonString {
    let codepoints = token.codepoints();
    let is_space = python_isspace;
    let start = codepoints
        .iter()
        .position(|code| !is_space(*code))
        .unwrap_or(codepoints.len());
    let end = codepoints
        .iter()
        .rposition(|code| !is_space(*code))
        .map(|position| position + 1)
        .unwrap_or(start);
    PythonString::from_codepoints(&codepoints[start..end])
        .expect("a slice of valid code points is valid")
}

/// The migration runbook the reference's retired-field refusal names.
const RETIRED_PACK_FIELD_RUNBOOK: &str = "docs/migrations/charter-pack-cutover.md";

/// Validate and normalise an `activations[*].artifact_kind` value.
///
/// The reference model accepts an [`ArtifactKind`] plural, or one of the
/// hand-authorable singular operator tokens (`CHARTER_KIND_TOKENS`), and stores
/// the canonical plural (`normalize_artifact_kind`). Both sets are derived from
/// the single kind authority — no list is copied here.
fn activation_kind(
    value: &Value,
    markers: &BTreeMap<String, PythonString>,
    path: &Path,
) -> Result<PythonString, DoctrineError> {
    let invalid = |detail: &str| DoctrineError::InvalidOrgCharter {
        path: path.to_path_buf(),
        message: format!("activations `artifact_kind` {detail}"),
    };
    let token = exact_python_str(value, markers).ok_or_else(|| invalid("must be a string"))?;
    let text = token
        .to_scalar_string()
        .map_err(|_| invalid("must be a scalar charter kind token"))?;
    let kind = ArtifactKind::from_plural(&text).or_else(|_| {
        ArtifactKind::from_operator_token(&text).and_then(|kind| {
            if sk_charter::CHARTER_KIND_TOKENS.contains(&kind.operator_token()) {
                Ok(kind)
            } else {
                Err(sk_charter::CharterError::UnknownArtifactKind {
                    token: text.clone(),
                    valid: sk_charter::CHARTER_KIND_TOKENS.join(", "),
                })
            }
        })
    });
    let kind = kind.map_err(|_| invalid("is not a known charter kind"))?;
    Ok(PythonString::from_scalars(kind.plural()))
}

/// The `required_<kind>` list of one constructed document, strictly.
fn required_list(
    path: &Path,
    mapping: &Mapping,
    markers: &BTreeMap<String, PythonString>,
    key: &str,
) -> Result<Vec<PythonString>, DoctrineError> {
    let Some(value) = mapping.get(key) else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let Some(items) = value.as_sequence() else {
        return Err(DoctrineError::InvalidOrgCharter {
            path: path.to_path_buf(),
            message: format!(
                "`{key}` in the org-charter.yaml must be a list of ids, got {}",
                describe(value)
            ),
        });
    };
    let mut out = Vec::new();
    for item in items {
        let Some(token) = exact_python_str(item, markers) else {
            return Err(DoctrineError::InvalidOrgCharter {
                path: path.to_path_buf(),
                message: format!("`{key}` must contain string ids only"),
            });
        };
        union_exact_tokens(&mut out, [token]);
    }
    Ok(out)
}

/// Read one pack's `org-charter.yaml`; `None` only when the file is absent
/// (or empty).
///
/// A present file with an unknown top-level key, a non-list `required_*`, or a
/// schema version outside 1..=2 is [`DoctrineError::InvalidOrgCharter`]: an
/// author who wrote a policy expects it to take effect, never to be dropped.
///
/// This is the **typed** whole-policy capability. The document read itself is the
/// raw owner's (`source::read_mapping`); schema validation happens here.
pub fn load_org_charter_policy(
    pack_root: &Path,
) -> Result<Option<OrgCharterPolicy>, DoctrineError> {
    let path = pack_root.join(ORG_CHARTER);
    if !path.is_file() {
        return Ok(None);
    }
    let Some((mapping, markers)) = source::read_mapping(&path)? else {
        return Ok(None);
    };

    let mut allowed: BTreeSet<String> = [
        "schema_version",
        "extends",
        "org_name",
        "interview_defaults",
        "skill_namespace",
        "governance_policies",
        "activations",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for kind in org_requirable_kinds() {
        allowed.insert(required_key(kind));
    }
    for key in mapping.keys() {
        let Some(key) = key.as_str() else {
            return Err(DoctrineError::InvalidOrgCharter {
                path,
                message: "org-charter.yaml keys must be strings".to_owned(),
            });
        };
        if !allowed.contains(key) {
            return Err(DoctrineError::InvalidOrgCharter {
                path,
                message: format!(
                    "unknown org-charter field {key:?}; allowed fields: {}",
                    allowed.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
            });
        }
    }

    let digit_limit = integer_digit_limit();
    let schema_version = match mapping.get("schema_version") {
        None | Some(Value::Null) => PythonInt::from_i64(ORG_CHARTER_SCHEMA_VERSION),
        Some(value @ Value::String(_)) => {
            let token = exact_python_str(value, &markers).expect("a string node is a string");
            PythonInt::parse_text(&token, digit_limit).ok_or_else(|| {
                DoctrineError::InvalidOrgCharter {
                    path: path.clone(),
                    message: "schema_version must be an integer or numeric string".to_owned(),
                }
            })?
        }
        Some(value) => match PythonInt::admit_yaml(value, digit_limit) {
            Ok(Some(version)) => version,
            // An integer literal beyond the interpreter's conversion ceiling
            // makes the reference's YAML load raise, so the whole document is
            // unreadable. Its observable is the same `None` an absent/parse-
            // failed document yields, never a schema refusal.
            Ok(None) => return Ok(None),
            Err(()) => {
                return Err(DoctrineError::InvalidOrgCharter {
                    path,
                    message: format!("schema_version must be an integer, got {}", describe(value)),
                });
            }
        },
    };
    // The pinned reference model types `schema_version` as a Python `int` and
    // folds ANY value (it is not a compatibility gate); refusing a higher version
    // would be a new validity refusal relative to upstream. Retired-field
    // rejection and the activation-entry shape still apply, so the field remains
    // meaningful. Its width and spelling are the exact `PythonInt` above.

    // The reference model keeps every `str`-typed field's authored spelling
    // verbatim (no strip, no empty-collapse); the fold applies the strip and the
    // last-non-empty rule to `skill_namespace`, and truthiness to `org_name`.
    let exact_text = |key: &str| {
        mapping
            .get(key)
            .and_then(|value| exact_python_str(value, &markers))
    };
    let extends = match mapping.get("extends") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            exact_python_str(value, &markers)
                .and_then(|token| token.to_scalar_string().ok())
                .ok_or_else(|| DoctrineError::InvalidOrgCharter {
                    path: path.clone(),
                    message: format!("extends must be a string, got {}", describe(value)),
                })?,
        ),
    };
    let org_name = exact_text("org_name");
    let skill_namespace = exact_text("skill_namespace");
    let mut interview_defaults = AuthoredDefaults::new();
    if let Some(defaults) = mapping.get("interview_defaults") {
        let Some(defaults) = defaults.as_mapping() else {
            return Err(DoctrineError::InvalidOrgCharter {
                path,
                message: "interview_defaults must be a mapping".to_owned(),
            });
        };
        for (key, value) in defaults {
            let Some(key) = exact_python_str(key, &markers) else {
                return Err(DoctrineError::InvalidOrgCharter {
                    path: path.clone(),
                    message: "interview_defaults keys must be strings".to_owned(),
                });
            };
            let value = AuthoredScalar::admit(value, &markers).ok_or_else(|| {
                DoctrineError::InvalidOrgCharter {
                    path: path.clone(),
                    message: format!("interview_defaults[{key}] must be a string or boolean"),
                }
            })?;
            interview_defaults.insert(key, value);
        }
    }

    let mut required = BTreeMap::new();
    for kind in org_requirable_kinds() {
        required.insert(
            kind,
            required_list(&path, &mapping, &markers, &required_key(kind))?,
        );
    }

    let mut governance_policies = Vec::new();
    if let Some(items) = mapping.get("governance_policies") {
        let Some(items) = items.as_sequence() else {
            return Err(DoctrineError::InvalidOrgCharter {
                path,
                message: "governance_policies must be a list".to_owned(),
            });
        };
        for item in items {
            let Some(entry) = item.as_mapping() else {
                return Err(DoctrineError::InvalidOrgCharter {
                    path: path.clone(),
                    message: "governance_policies entries must be mappings".to_owned(),
                });
            };
            // `GovernancePolicy` is `extra="forbid"` in the reference model.
            let field = match entry.get("field") {
                Some(value) => exact_python_str(value, &markers).ok_or_else(|| {
                    DoctrineError::InvalidOrgCharter {
                        path: path.clone(),
                        message: format!(
                            "governance_policies `field` must be a string, got {}",
                            describe(value)
                        ),
                    }
                })?,
                None => {
                    return Err(DoctrineError::InvalidOrgCharter {
                        path: path.clone(),
                        message: "governance_policies entry is missing `field`".to_owned(),
                    });
                }
            };
            for key in entry.keys() {
                let Some(key) = key.as_str() else {
                    return Err(DoctrineError::InvalidOrgCharter {
                        path: path.clone(),
                        message: "governance_policies entry keys must be strings".to_owned(),
                    });
                };
                if !["field", "value", "enforcement"].contains(&key) {
                    return Err(DoctrineError::InvalidOrgCharter {
                        path: path.clone(),
                        message: format!(
                            "unknown governance_policies field {key:?}; allowed fields: enforcement, field, value"
                        ),
                    });
                }
            }
            let value = match entry.get("value") {
                Some(value) => AuthoredScalar::admit(value, &markers).ok_or_else(|| {
                    DoctrineError::InvalidOrgCharter {
                        path: path.clone(),
                        message: "governance_policies `value` must be a string or boolean"
                            .to_owned(),
                    }
                })?,
                None => {
                    return Err(DoctrineError::InvalidOrgCharter {
                        path: path.clone(),
                        message: "governance_policies `value` must be a string or boolean"
                            .to_owned(),
                    });
                }
            };
            let enforcement = match entry.get("enforcement") {
                None => PythonString::from_scalars("advisory"),
                Some(value) => exact_python_str(value, &markers).ok_or_else(|| {
                    DoctrineError::InvalidOrgCharter {
                        path: path.clone(),
                        message: format!(
                            "governance_policies `enforcement` must be a string, got {}",
                            describe(value)
                        ),
                    }
                })?,
            };
            governance_policies.push(GovernancePolicy {
                field,
                value,
                enforcement,
            });
        }
    }

    // CF58: the reference's `ActivationEntry` model validator (`_validate_context`)
    // constrains the two closed selectors on `activation_context` —
    // `mission_type` against `ALLOWED_MISSION_TYPES` and `action` against
    // `REGISTERED_TRIGGERS` plus the wildcards. A violation is a pydantic
    // `ValidationError`, i.e. an INVALID PACK DOCUMENT the typed fold skips; it is
    // never a refusal (unlike the retired-field check above). The roster is
    // the built-in mission-type document set the reference derives
    // `ALLOWED_MISSION_TYPES` from, built once for a document that declares
    // entries and only then (`crate::activation_context` owns the vocabularies).
    let selector_registry = match mapping.get("activations") {
        Some(Value::Sequence(items)) if !items.is_empty() => {
            Some(crate::activation_context::ActivationSelectorRegistry::canonical()?)
        }
        _ => None,
    };

    let mut activations = Vec::new();
    if let Some(items) = mapping.get("activations") {
        let Some(items) = items.as_sequence() else {
            return Err(DoctrineError::InvalidOrgCharter {
                path,
                message: "activations must be a list".to_owned(),
            });
        };
        for item in items {
            let Some(entry) = item.as_mapping() else {
                return Err(DoctrineError::InvalidOrgCharter {
                    path: path.clone(),
                    message: "activations entries must be mappings".to_owned(),
                });
            };
            // `ActivationEntry` is `extra="forbid"` and rejects the retired
            // `doctrine_pack_id` by name (the reference's `RETIRED_PACK_FIELD`).
            for key in entry.keys() {
                let Some(key) = key.as_str() else {
                    return Err(DoctrineError::InvalidOrgCharter {
                        path: path.clone(),
                        message: "activations entry keys must be strings".to_owned(),
                    });
                };
                if key == "doctrine_pack_id" {
                    return Err(DoctrineError::OrgCharterRetiredField {
                        path: path.clone(),
                        field: "doctrine_pack_id".to_owned(),
                        replacement: "charter_pack_id".to_owned(),
                        runbook: RETIRED_PACK_FIELD_RUNBOOK,
                    });
                }
                if ![
                    "activation_context",
                    "charter_pack_id",
                    "artifact_id",
                    "artifact_kind",
                ]
                .contains(&key)
                {
                    return Err(DoctrineError::InvalidOrgCharter {
                        path: path.clone(),
                        message: format!(
                            "unknown activations field {key:?}; allowed fields: activation_context, artifact_id, artifact_kind, charter_pack_id"
                        ),
                    });
                }
            }
            let Some(context) = entry.get("activation_context") else {
                return Err(DoctrineError::InvalidOrgCharter {
                    path: path.clone(),
                    message: "activations entry is missing `activation_context`".to_owned(),
                });
            };
            let Some(context) = context.as_mapping() else {
                return Err(DoctrineError::InvalidOrgCharter {
                    path: path.clone(),
                    message: "activations `activation_context` must be a mapping".to_owned(),
                });
            };
            let mut pairs = Vec::with_capacity(context.len());
            for (key, value) in context {
                let (Some(key), Some(value)) = (
                    exact_python_str(key, &markers),
                    exact_python_str(value, &markers),
                ) else {
                    return Err(DoctrineError::InvalidOrgCharter {
                        path: path.clone(),
                        message: "activations `activation_context` must map strings to strings"
                            .to_owned(),
                    });
                };
                pairs.push((key, value));
            }
            // The reference validates `activation_context` before the remaining
            // entry fields, so a bad selector wins over a missing
            // `charter_pack_id` / unknown `artifact_kind`.
            if let Some(registry) = &selector_registry
                && let Err(rejection) = registry.validate_context(&pairs)
            {
                return Err(DoctrineError::InvalidOrgCharter {
                    path: path.clone(),
                    message: rejection.message(registry),
                });
            }
            let field = |name: &str| -> Result<PythonString, DoctrineError> {
                entry
                    .get(name)
                    .and_then(|value| exact_python_str(value, &markers))
                    .ok_or_else(|| DoctrineError::InvalidOrgCharter {
                        path: path.clone(),
                        message: format!("activations entry is missing `{name}`"),
                    })
            };
            let artifact_kind = match entry.get("artifact_kind") {
                None | Some(Value::Null) => None,
                Some(value) => Some(activation_kind(value, &markers, &path)?),
            };
            activations.push(ActivationEntry {
                context: pairs,
                charter_pack_id: field("charter_pack_id")?,
                artifact_id: field("artifact_id")?,
                artifact_kind,
            });
        }
    }

    Ok(Some(OrgCharterPolicy {
        path,
        pack_root: pack_root.to_path_buf(),
        schema_version,
        extends,
        org_name,
        interview_defaults,
        required,
        skill_namespace,
        governance_policies,
        activations,
    }))
}

/// Union exact `items` into `into`: stripped, non-empty, first-seen order.
///
/// This is THE merge rule for every `required_<kind>` field. Comparison is
/// exact code-point equality, so a surrogate-bearing token is never confused
/// with its escaped spelling.
pub fn union_exact_tokens(
    into: &mut Vec<PythonString>,
    items: impl IntoIterator<Item = PythonString>,
) {
    for item in items {
        let token = strip_token(&item);
        if !token.is_empty() && !into.contains(&token) {
            into.push(token);
        }
    }
}

/// Union `items` into `into`: stripped, non-empty, first-seen order.
///
/// The scalar form of [`union_exact_tokens`]; `&str` input is always a valid
/// code-point sequence, so nothing is lost on this path.
pub fn union_required_tokens(
    into: &mut Vec<String>,
    items: impl IntoIterator<Item = impl AsRef<str>>,
) {
    let mut exact: Vec<PythonString> = into
        .iter()
        .map(|token| PythonString::from_scalars(token))
        .collect();
    union_exact_tokens(
        &mut exact,
        items
            .into_iter()
            .map(|item| PythonString::from_scalars(item.as_ref())),
    );
    into.clear();
    into.extend(
        exact
            .into_iter()
            .filter_map(|token| token.to_scalar_string().ok()),
    );
}

/// The configured org pack roots, in declaration order.
///
/// The registry and the path authority are the canonical
/// `charter_packs.org.packs[]` read (`source::declared_org_packs`); the
/// built-in tier is **not** consulted, because an unavailable built-in pack must
/// never erase a readable org policy (the reference's `_enumerate_org_pack_paths`
/// reads only the configured packs).
pub fn configured_org_roots(repo_root: &Path) -> Result<Vec<(String, PathBuf)>, DoctrineError> {
    source::declared_org_packs(repo_root, true)
}

/// Configured org pack roots, resolved against an explicit packs-root override.
///
/// `packs_root` is retained for API compatibility with the seams that thread a
/// `SPEC_KITTY_PACKS_ROOT` override (parent or built-in leaf) through every pack
/// read. The org tier itself is **registry-born**, exactly like the reference: it
/// never resolves through the built-in search, so neither spelling can change
/// (or erase) the org roots this returns.
pub fn configured_org_roots_with_packs_root(
    repo_root: &Path,
    _packs_root: Option<&Path>,
) -> Result<Vec<(String, PathBuf)>, DoctrineError> {
    configured_org_roots(repo_root)
}

pub(crate) fn strict_error(message: String) -> DoctrineError {
    DoctrineError::InvalidOrgCharter {
        path: PathBuf::from("org pack registry"),
        message: format!(
            "{message}\nThe org required floor cannot be read, so nothing was changed. \
             Fix the org pack, or run `spec-kitty doctor charter-packs`."
        ),
    }
}

/// Every `(declared pack, member pack, policy)` the raw walk visits.
///
/// The members are the lenient per-field projection of the raw documents
/// (`source`'s `OrgCharterRawDocument::to_policy`): the raw walk validates no whole
/// policy, so a malformed unrelated field cannot erase a pack's requirements or
/// its `extends:` contribution. A strict-posture refusal therefore comes from the
/// document/chain phases, and a malformed `required_<kind>` from the per-field
/// reader ([`read_org_required_tokens_exact`]), never from this projection.
pub fn org_charter_documents(
    repo_root: &Path,
    strict: bool,
) -> Result<Vec<(String, String, OrgCharterPolicy)>, DoctrineError> {
    org_charter_documents_with_packs_root(repo_root, strict, packs_root_override().as_deref())
}

/// [`org_charter_documents`] with the packs-root override retained for the
/// callers that thread it (the org walk itself is registry-born, so the spelling
/// is inert — see [`configured_org_roots_with_packs_root`]).
pub fn org_charter_documents_with_packs_root(
    repo_root: &Path,
    strict: bool,
    _packs_root: Option<&Path>,
) -> Result<Vec<(String, String, OrgCharterPolicy)>, DoctrineError> {
    let mut out = Vec::new();
    for document in source::walk(repo_root, strict)? {
        out.push((
            document.declared().to_owned(),
            document.member().to_owned(),
            document.to_policy()?,
        ));
    }
    Ok(out)
}

/// The **raw** org-charter walk: every declared pack and its `extends:` chain.
///
/// This is `org_pack_discovery._iter_org_charter_docs` — the single raw reader
/// behind the required floor, the `required_<kind>` overlay, the org activations
/// and the skill namespace. Each declared pack yields itself first
/// (`member == declared`), then the bases its `extends:` chain inherits, nearest
/// first. A caller that wants each pack once filters `declared == member`.
///
/// Lenient (`strict == false`): a pack whose `org-charter.yaml` is absent,
/// unreadable or not a mapping is skipped, and an invalid `extends:` chain falls
/// back to the **declared pack alone** — never a partially visited prefix.
/// Strict: an unreadable registry, a declared pack that is not a directory, an
/// unreadable or non-mapping document, and a cycling or dangling chain are all
/// typed refusals (in the reference's failure order: registry, per declared
/// document, then per declared chain).
///
/// A document that makes the reference's loader raise an **uncaught** Python
/// exception escapes every posture as
/// [`DoctrineError::OrgCharterReadAborted`]; it is never reclassified as a skip.
pub fn org_charter_raw_documents(
    repo_root: &Path,
    strict: bool,
) -> Result<Vec<OrgCharterRawDocument>, DoctrineError> {
    source::walk(repo_root, strict)
}

/// Each declared pack's own document once, declaration order.
pub fn declared_policies(
    repo_root: &Path,
    strict: bool,
) -> Result<Vec<(String, OrgCharterPolicy)>, DoctrineError> {
    Ok(org_charter_documents(repo_root, strict)?
        .into_iter()
        .filter(|(declared, member, _)| declared == member)
        .map(|(declared, _member, policy)| (declared, policy))
        .collect())
}

/// The required floor of `kind` as the raw ids the packs list — the **scalar
/// capability view**.
///
/// Tokens with no scalar projection are omitted from this view; use
/// [`read_org_required_tokens_exact`] for the full token list.
pub fn read_org_required_tokens(
    repo_root: &Path,
    kind: ArtifactKind,
    strict: bool,
) -> Result<Vec<String>, DoctrineError> {
    read_org_required_tokens_with_packs_root(
        repo_root,
        kind,
        strict,
        packs_root_override().as_deref(),
    )
}

/// [`read_org_required_tokens`] resolved against an explicit packs-root
/// override (parent or built-in leaf spelling).
pub fn read_org_required_tokens_with_packs_root(
    repo_root: &Path,
    kind: ArtifactKind,
    strict: bool,
    packs_root: Option<&Path>,
) -> Result<Vec<String>, DoctrineError> {
    Ok(
        read_org_required_tokens_exact_with_packs_root(repo_root, kind, strict, packs_root)?
            .into_iter()
            .filter_map(|token| token.to_scalar_string().ok())
            .collect(),
    )
}

/// The required floor of `kind` as the raw ids the packs list, exactly.
pub fn read_org_required_tokens_exact(
    repo_root: &Path,
    kind: ArtifactKind,
    strict: bool,
) -> Result<Vec<PythonString>, DoctrineError> {
    read_org_required_tokens_exact_with_packs_root(
        repo_root,
        kind,
        strict,
        packs_root_override().as_deref(),
    )
}

/// [`read_org_required_tokens_exact`] with the packs-root override retained for
/// the callers that thread it (the org walk is registry-born, so the spelling is
/// inert).
///
/// This is `org_pack_discovery.read_org_required_tokens`: the raw walk, then the
/// **per requested field** validation (see
/// [`OrgCharterRawDocument::requirement_tokens`]).
/// A malformed `required_<other>` field cannot poison `kind`; a malformed
/// `required_<kind>` is a strict refusal and a lenient skip.
pub fn read_org_required_tokens_exact_with_packs_root(
    repo_root: &Path,
    kind: ArtifactKind,
    strict: bool,
    _packs_root: Option<&Path>,
) -> Result<Vec<PythonString>, DoctrineError> {
    let mut out = Vec::new();
    for document in source::walk(repo_root, strict)? {
        union_exact_tokens(&mut out, document.requirement_tokens(kind, strict)?);
    }
    Ok(out)
}

/// The org-required ids of `kind` **coerced through Python `str()`**, exactly.
///
/// This is `org_pack_discovery.read_org_required_ids`, the lenient, coercing
/// reader that serves `required_skills` (no `selected_skills` overlay) and the
/// absent-key "in force" default. It is deliberately separate from
/// [`read_org_required_tokens_exact`]: a `required_<kind>: [1, " A ", true]`
/// yields `["1", "A", "True"]` here and NOTHING there, because the reference's
/// two readers answer different questions. A non-list value contributes nothing
/// and never refuses.
pub fn read_org_required_ids_exact(
    repo_root: &Path,
    kind: ArtifactKind,
) -> Result<Vec<PythonString>, DoctrineError> {
    let mut out = Vec::new();
    for document in source::walk(repo_root, false)? {
        union_exact_tokens(&mut out, document.coerced_requirement_tokens(kind));
    }
    Ok(out)
}

/// [`read_org_required_ids_exact`] as the **scalar capability view**.
///
/// Tokens with no scalar projection (a lone surrogate survives `str()` and
/// `strip()`) are omitted here; the exact list keeps them. The coercing rule is
/// never weakened for this view.
pub fn read_org_required_ids(
    repo_root: &Path,
    kind: ArtifactKind,
) -> Result<Vec<String>, DoctrineError> {
    Ok(read_org_required_ids_exact(repo_root, kind)?
        .into_iter()
        .filter_map(|token| token.to_scalar_string().ok())
        .collect())
}

/// Union every org pack's `required_<kind>` for the selection-overlayable kinds,
/// exactly (`_read_org_required_selections`).
///
/// The kinds are [`selection_overlayable_kinds`] — `glossary_pack`, `asset` and
/// `skill` are org-requirable but have no `selected_<kind>` overlay, so they are
/// absent here by construction (they are read by [`read_org_required_ids_exact`]).
pub fn read_org_required_selections_exact(
    repo_root: &Path,
) -> Result<BTreeMap<ArtifactKind, Vec<PythonString>>, DoctrineError> {
    let mut out: BTreeMap<ArtifactKind, Vec<PythonString>> = selection_overlayable_kinds()
        .iter()
        .map(|kind| (*kind, Vec::new()))
        .collect();
    // Source walks/reads each document once, then projects all selection kinds.
    for document in source::walk(repo_root, false)? {
        for (kind, tokens) in &mut out {
            union_exact_tokens(tokens, document.coerced_requirement_tokens(*kind));
        }
    }
    Ok(out)
}

/// [`read_org_required_selections_exact`] as the **scalar capability view**.
pub fn read_org_required_selections(
    repo_root: &Path,
) -> Result<BTreeMap<ArtifactKind, Vec<String>>, DoctrineError> {
    Ok(read_org_required_selections_exact(repo_root)?
        .into_iter()
        .map(|(kind, tokens)| {
            (
                kind,
                tokens
                    .into_iter()
                    .filter_map(|token| token.to_scalar_string().ok())
                    .collect(),
            )
        })
        .collect())
}

/// The required floor with the packs that require each normalized stem.
pub fn read_org_required_provenance(
    repo_root: &Path,
    kind: ArtifactKind,
    identity: &IdentityIndex,
    strict: bool,
) -> Result<BTreeMap<String, Vec<RequiringPack>>, DoctrineError> {
    read_org_required_provenance_with_packs_root(
        repo_root,
        kind,
        identity,
        strict,
        packs_root_override().as_deref(),
    )
}

/// [`read_org_required_provenance`] with the packs-root override retained for the
/// callers that thread it.
///
/// The stem keys are the **scalar capability view** of
/// [`read_org_required_provenance_exact`]: an unresolved token with no scalar
/// projection is omitted from this map (never invented, never lossily
/// re-spelled), and the exact map keeps it.
pub fn read_org_required_provenance_with_packs_root(
    repo_root: &Path,
    kind: ArtifactKind,
    identity: &IdentityIndex,
    strict: bool,
    _packs_root: Option<&Path>,
) -> Result<BTreeMap<String, Vec<RequiringPack>>, DoctrineError> {
    let mut provenance: BTreeMap<String, Vec<RequiringPack>> = BTreeMap::new();
    for (stem, packs) in read_org_required_provenance_exact(repo_root, kind, identity, strict)? {
        let Ok(stem) = stem.to_scalar_string() else {
            continue;
        };
        provenance.insert(stem, packs);
    }
    Ok(provenance)
}

/// The required floor of `kind` with provenance, **exactly**.
///
/// `stem -> the packs that require it`, first-seen order, from the one raw walk
/// and the same per-field reader as [`read_org_required_tokens_exact`].
///
/// Id normalization is the reference reader's own: a known id maps to its
/// activation stem, an id that resolves in no layer is kept as written, and a
/// declared id whose file name another layer uses for a different id is kept as
/// written too — the reference's reader catches that collision (only its
/// *writers* refuse it), so a deny can never remove the id and no token is
/// dropped. Nothing here is lossy: keys are exact code points.
pub fn read_org_required_provenance_exact(
    repo_root: &Path,
    kind: ArtifactKind,
    identity: &IdentityIndex,
    strict: bool,
) -> Result<BTreeMap<PythonString, Vec<RequiringPack>>, DoctrineError> {
    let mut provenance: BTreeMap<PythonString, Vec<RequiringPack>> = BTreeMap::new();
    for document in source::walk(repo_root, strict)? {
        let ids = document.requirement_tokens(kind, strict)?;
        if ids.is_empty() {
            continue;
        }
        let requirer = RequiringPack {
            pack: document.member().to_owned(),
            via: (document.member() != document.declared()).then(|| document.declared().to_owned()),
        };
        for token in ids {
            let stem = required_stem(identity, kind, &token)?;
            let entries = provenance.entry(stem).or_default();
            if !entries.contains(&requirer) {
                entries.push(requirer.clone());
            }
        }
    }
    Ok(provenance)
}

/// The reference reader's normalization: a stem, else the id as written.
///
/// A collision is caught (the reference's `_stem_normaliser` catches
/// `ActivationIdCollisionError` for readers); every other identity error still
/// propagates.
fn required_stem(
    identity: &IdentityIndex,
    kind: ArtifactKind,
    token: &PythonString,
) -> Result<PythonString, DoctrineError> {
    match identity.stored_spelling_exact(kind, token) {
        Ok(stem) => Ok(stem),
        Err(DoctrineError::ActivationIdCollision { .. }) => Ok(token.clone()),
        Err(error) => Err(error),
    }
}

/// The org-required ids of `kind` as activation stems (the `--only` and
/// promotion writers' spelling), exactly.
///
/// The input is the **typed cross-pack fold** ([`fold::fold_org_charter_policies`]),
/// exactly like the reference's `org_required_stems` — NOT the raw per-field walk:
/// a pack the typed schema refuses contributes nothing here, while a pack the
/// typed model accepts contributes its whole `required_<kind>` list. The raw
/// readers ([`read_org_required_tokens_exact`] and friends) keep their own,
/// deliberately different role for diagnostics.
///
/// An id that resolves in no readable layer is kept raw and named in
/// `warnings`; a declared id whose file name another layer uses for a different
/// id raises [`DoctrineError::ActivationIdCollision`] (this is a WRITER path, so
/// the reference's collision refusal applies).
pub fn org_required_stems(
    repo_root: &Path,
    kind: ArtifactKind,
    identity: &IdentityIndex,
    warnings: &mut Vec<String>,
) -> Result<Vec<PythonString>, DoctrineError> {
    if !org_requirable_kinds().contains(&kind) {
        return Ok(Vec::new());
    }
    let policy = fold::fold_org_charter_policies(repo_root)?;
    let mut stems = Vec::new();
    for token in policy.required_for(kind) {
        if !identity.knows_exact(kind, token) {
            warnings.push(format!(
                "Org-required {} {} was not found in any readable layer; kept as written.",
                kind.canonical(),
                token.repr()
            ));
        }
        union_exact_tokens(&mut stems, [identity.stored_spelling_exact(kind, token)?]);
    }
    Ok(stems)
}

/// The whole required floor, unioned across packs and normalized to stems.
pub fn required_floor(
    repo_root: &Path,
    identity: &IdentityIndex,
) -> Result<RequiredFloor, DoctrineError> {
    required_floor_with_packs_root(repo_root, identity, packs_root_override().as_deref())
}

/// [`required_floor`] with the packs-root override retained for the callers that
/// thread it (the org registry read is registry-born, so the built-in spelling is
/// inert).
pub fn required_floor_with_packs_root(
    repo_root: &Path,
    identity: &IdentityIndex,
    _packs_root: Option<&Path>,
) -> Result<RequiredFloor, DoctrineError> {
    let mut floor = RequiredFloor::default();
    for kind in org_requirable_kinds() {
        for token in read_org_required_tokens_exact(repo_root, kind, false)? {
            if !identity.knows_exact(kind, &token) && !floor.unresolved_exact.contains(&token) {
                floor.unresolved_exact.push(token.clone());
                floor
                    .unresolved
                    .push(token.to_scalar_string().unwrap_or_else(|_| token.repr()));
            }
            let stem = required_stem(identity, kind, &token)?;
            // Stems are file names; an unknown token has no stem and cannot
            // match a scalar activation id either, so the floor map holds its
            // scalar spelling for comparison purposes.
            let Ok(stem) = stem.to_scalar_string() else {
                continue;
            };
            let entries = floor.tokens.entry(kind).or_default();
            if !entries.contains(&stem) {
                entries.push(stem);
            }
        }
    }
    for kind in org_requirable_kinds() {
        for (stem, packs) in
            read_org_required_provenance_with_packs_root(repo_root, kind, identity, false, None)?
        {
            floor.provenance.insert((kind, stem), packs);
        }
    }
    Ok(floor)
}

/// Refuse a required directive id that resolves in no readable layer.
///
/// The org charter's `required_directives` are a hard floor; a typo there must
/// not silently shrink it.
pub fn validate_org_required_directive_stems(
    repo_root: &Path,
    identity: &IdentityIndex,
) -> Result<(), DoctrineError> {
    let tokens = read_org_required_tokens_exact(repo_root, ArtifactKind::Directive, false)?;
    let unknown: Vec<String> = tokens
        .into_iter()
        .filter(|token| !identity.knows_exact(ArtifactKind::Directive, token))
        .map(|token| token.repr())
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    Err(DoctrineError::InvalidOrgCharter {
        path: PathBuf::from(ORG_CHARTER),
        message: format!(
            "required_directives names directive(s) that resolve in no readable layer: {}. \
             Correct the org-charter.yaml, or fetch the pack that ships them.",
            unknown.join(", ")
        ),
    })
}

/// The org-tier `expected-artifacts.yaml` of `mission_type`, whole-file, the
/// last matching org root wins.
pub fn resolve_org_expected_artifacts(
    org_roots: &[PathBuf],
    mission_type: &str,
) -> Result<Option<Value>, DoctrineError> {
    resolve_org_manifest(org_roots, mission_type, "expected-artifacts.yaml")
}

/// The org-tier `requirement-kinds.yaml` of `mission_type`, whole-file, the
/// last matching org root wins.
pub fn resolve_org_requirement_kinds(
    org_roots: &[PathBuf],
    mission_type: &str,
) -> Result<Option<Value>, DoctrineError> {
    resolve_org_manifest(org_roots, mission_type, "requirement-kinds.yaml")
}

fn resolve_org_manifest(
    org_roots: &[PathBuf],
    mission_type: &str,
    file_name: &str,
) -> Result<Option<Value>, DoctrineError> {
    let mut result = None;
    for root in org_roots {
        let path = root.join("missions").join(mission_type).join(file_name);
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|source| DoctrineError::io("read org charter", &path, source))?;
        let value: Value =
            serde_yaml_ng::from_str(&text).map_err(|error| DoctrineError::InvalidOrgCharter {
                path: path.clone(),
                message: error.to_string(),
            })?;
        if !value.is_mapping() {
            return Err(DoctrineError::InvalidOrgCharter {
                path,
                message: format!("{file_name} must be a YAML mapping"),
            });
        }
        result = Some(value);
    }
    Ok(result)
}

/// The org `skill_namespace`, last non-empty value across packs wins, exactly.
///
/// `read_org_skill_namespace`'s law over the raw walk's **declared** documents
/// only (`_declared_docs`: a base pack's namespace is not inherited): a string
/// value, stripped, empty ignored; the last non-empty one wins. No coercion, so
/// `skill_namespace: 5` contributes nothing.
///
/// Known debt: the reference also runs the namespace grammar check while reading
/// (`require_valid_skill_namespace`, per encountered value). Native leaves that
/// check to the pack-skill consumer (`skill_namespace_violation`), so an invalid
/// namespace is refused there rather than here, and an invalid earlier value that
/// a later pack overrides is not refused at all. The grammar owner is the Skill
/// capability lane, not this reader.
pub fn read_org_skill_namespace_exact(
    repo_root: &Path,
) -> Result<Option<PythonString>, DoctrineError> {
    let mut namespace: Option<PythonString> = None;
    for document in source::walk(repo_root, false)? {
        if document.declared() != document.member() {
            continue;
        }
        if let Some(candidate) = document.skill_namespace() {
            namespace = Some(candidate);
        }
        // `read_org_skill_namespace` validates every value it *encounters*, not
        // only the final one: an invalid namespace in an earlier declared pack is
        // refused even when a later pack overrides it. The remedy text names the
        // pack whose document was being read, exactly like the reference's
        // `f"fix `skill_namespace` in org pack {name!r}'s org-charter.yaml"`.
        if let Some(current) = &namespace
            && let Some(violation) =
                crate::pack_skill_repository::skill_namespace_violation_exact(current)
        {
            let pack = document.member().to_owned();
            let remedy = format!(
                "fix `skill_namespace` in org pack {}'s org-charter.yaml",
                PythonString::from_scalars(&pack).repr()
            );
            return Err(DoctrineError::OrgSkillNamespaceInvalid {
                pack,
                message: format!("{violation}; {remedy}"),
            });
        }
    }
    Ok(namespace)
}

/// The org `skill_namespace`, last non-empty value across packs wins.
///
/// The explicit **scalar capability view**: a namespace with no scalar
/// projection is `None` here (the exact value stays on
/// [`read_org_skill_namespace_exact`]).
pub fn read_org_skill_namespace(repo_root: &Path) -> Result<Option<String>, DoctrineError> {
    Ok(read_org_skill_namespace_exact(repo_root)?.and_then(|token| token.to_scalar_string().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn config(root: &Path, body: &str) {
        write(&root.join(".kittify/config.yaml"), body);
    }

    fn pack(root: &Path, name: &str) -> PathBuf {
        let path = root.join("packs").join(name);
        if name != "built-in" {
            write(
                &root.join("packs/built-in/pack.yaml"),
                "name: built-in\npack_level: built-in\n",
            );
        }
        path
    }

    fn scalars(tokens: &[PythonString]) -> Vec<String> {
        tokens
            .iter()
            .map(|token| token.to_scalar_string().unwrap())
            .collect()
    }

    #[test]
    fn org_floor_read_does_not_require_a_built_in_pack() {
        // The org tier is registry-born (reference `_enumerate_org_pack_paths`
        // never reads the built-in tier). Routing the floor through the built-in
        // search made an unavailable built-in pack erase a readable org floor;
        // the packs root here has no `built-in/` leaf at all.
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let packstore = root.join("packstore");
        let acme = packstore.join("acme");
        write(&acme.join(ORG_CHARTER), "required_directives: [D1]\n");
        config(
            root,
            &format!(
                "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
                acme.display()
            ),
        );
        assert!(!packstore.join("built-in").exists());
        for strict in [false, true] {
            assert_eq!(
                scalars(
                    &read_org_required_tokens_exact_with_packs_root(
                        root,
                        ArtifactKind::Directive,
                        strict,
                        Some(&packstore)
                    )
                    .unwrap()
                ),
                vec!["D1".to_owned()],
                "strict={strict}"
            );
        }
        assert_eq!(org_charter_raw_documents(root, true).unwrap().len(), 1);
    }

    #[test]
    fn strip_token_matches_python_str_strip_for_information_separators() {
        // `str.isspace()` is `White_Space` plus U+001C..=U+001F; `char::is_whitespace`
        // excludes the four information separators, so they are added explicitly.
        let token = PythonString::from_codepoints(&[0x1c, 0x20, 0x41, 0x1d]).unwrap();
        assert_eq!(strip_token(&token).to_scalar_string().unwrap(), "A");
        let untouched = PythonString::from_codepoints(&[0x41, 0x2d, 0x42]).unwrap();
        assert_eq!(strip_token(&untouched), untouched);
    }

    #[test]
    fn parses_required_lists_and_rejects_bad_shapes() {
        let directory = tempfile::tempdir().unwrap();
        let root = pack(directory.path(), "acme");
        write(
            &root.join(ORG_CHARTER),
            "schema_version: 2\norg_name: Acme\nrequired_directives: [DIRECTIVE_001, ' 001-arch ']\nrequired_skills: [s1]\ninterview_defaults:\n  strict: true\n",
        );
        let policy = load_org_charter_policy(&root).unwrap().unwrap();
        assert_eq!(
            scalars(policy.required_for(ArtifactKind::Directive)),
            ["DIRECTIVE_001".to_owned(), "001-arch".to_owned()]
        );
        assert_eq!(
            scalars(policy.required_for(ArtifactKind::Skill)),
            ["s1".to_owned()]
        );
        assert_eq!(
            policy
                .interview_defaults
                .get(&PythonString::from_scalars("strict"))
                .map(|value| value.python_str().to_scalar_string().unwrap()),
            Some("True".to_owned())
        );

        write(&root.join(ORG_CHARTER), "required_directives: 7\n");
        assert!(matches!(
            load_org_charter_policy(&root),
            Err(DoctrineError::InvalidOrgCharter { .. })
        ));
        write(&root.join(ORG_CHARTER), "mystery: []\n");
        assert!(matches!(
            load_org_charter_policy(&root),
            Err(DoctrineError::InvalidOrgCharter { .. })
        ));
        // The reference model types `schema_version` as an int and folds ANY
        // value; a higher version is read, not refused.
        write(&root.join(ORG_CHARTER), "schema_version: 9\n");
        assert_eq!(
            load_org_charter_policy(&root)
                .unwrap()
                .unwrap()
                .schema_version
                .as_decimal(),
            "9"
        );
    }

    #[test]
    fn a_non_scalar_requirement_token_is_read_exactly() {
        // The reference's ruamel reader carries `"author\ud800"` as a Python
        // str; this reader carries the identical code points instead of
        // refusing the pack.
        let directory = tempfile::tempdir().unwrap();
        let root = pack(directory.path(), "acme");
        write(
            &root.join(ORG_CHARTER),
            "required_agent_profiles: [\"author\\ud800\", ok-id]\n",
        );
        let policy = load_org_charter_policy(&root).unwrap().unwrap();
        let tokens = policy.required_for(ArtifactKind::AgentProfile);
        assert_eq!(tokens.len(), 2);
        assert_eq!(
            tokens[0].codepoints(),
            &[0x61, 0x75, 0x74, 0x68, 0x6f, 0x72, 0xd800]
        );
        assert_eq!(tokens[1].to_scalar_string().unwrap(), "ok-id");
        assert_eq!(
            policy.required_scalar(ArtifactKind::AgentProfile),
            vec!["ok-id".to_owned()],
            "the scalar view names its capability boundary without inventing a token"
        );
    }

    #[test]
    fn lenient_floor_keeps_every_other_kind_and_the_extends_chain() {
        // F4: one malformed `required_*` field must not discard the pack's other
        // requirements or the `extends:` contribution. The reference reads the
        // raw mapping and validates each `required_<kind>` independently.
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let base = pack(root, "base");
        let overlay = pack(root, "overlay");
        write(
            &base.join(ORG_CHARTER),
            "required_directives: [DIRECTIVE_001]\n",
        );
        write(
            &overlay.join(ORG_CHARTER),
            "extends: base\nrequired_directives: DIRECTIVE_003\nrequired_tactics: [acceptance-test-first]\n",
        );
        config(
            root,
            &format!(
                "charter_packs:\n  org:\n    packs:\n      - name: base\n        local_path: {}\n      - name: overlay\n        local_path: {}\n",
                base.display(),
                overlay.display()
            ),
        );
        // The pinned reference's raw DOCUMENT walk validates no field at all
        // (`_iter_org_charter_docs` only reads the raw mapping), so the strict
        // document walk succeeds here; the refusal belongs to the per-requested
        // *field* reader, which is the floor.
        assert_eq!(org_charter_documents(root, true).unwrap().len(), 3);
        assert!(
            read_org_required_tokens(root, ArtifactKind::Directive, true)
                .unwrap_err()
                .to_string()
                .contains("required_directives")
        );
        let directives = read_org_required_tokens(root, ArtifactKind::Directive, false).unwrap();
        assert_eq!(directives, ["DIRECTIVE_001"]);
        let tactics = read_org_required_tokens(root, ArtifactKind::Tactic, false).unwrap();
        assert_eq!(tactics, ["acceptance-test-first"]);
    }

    #[test]
    fn floor_unions_packs_and_extends_chains_with_provenance() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let base = pack(root, "base");
        let overlay = pack(root, "overlay");
        write(
            &base.join(ORG_CHARTER),
            "required_directives: [DIRECTIVE_001]\n",
        );
        write(
            &overlay.join(ORG_CHARTER),
            "extends: base\nrequired_directives: [DIRECTIVE_002, DIRECTIVE_001]\n",
        );
        write(
            &base.join("directives/001-arch.directive.yaml"),
            "id: DIRECTIVE_001\n",
        );
        write(
            &base.join("directives/002-style.directive.yaml"),
            "id: DIRECTIVE_002\n",
        );
        config(
            root,
            &format!(
                "charter_packs:\n  org:\n    packs:\n      - name: base\n        local_path: {}\n      - name: overlay\n        local_path: {}\n",
                base.display(),
                overlay.display()
            ),
        );
        let identity = IdentityIndex::from_roots(
            &root.join("packs/built-in"),
            &[base.clone(), overlay.clone()],
            None,
        );
        let tokens = read_org_required_tokens(root, ArtifactKind::Directive, true).unwrap();
        assert_eq!(tokens, ["DIRECTIVE_001", "DIRECTIVE_002"]);
        let provenance =
            read_org_required_provenance(root, ArtifactKind::Directive, &identity, true).unwrap();
        assert_eq!(
            provenance["001-arch"],
            vec![
                RequiringPack {
                    pack: "base".to_owned(),
                    via: None
                },
                RequiringPack {
                    pack: "overlay".to_owned(),
                    via: None
                },
                RequiringPack {
                    pack: "base".to_owned(),
                    via: Some("overlay".to_owned())
                },
            ]
        );
        assert_eq!(
            provenance["002-style"],
            vec![RequiringPack {
                pack: "overlay".to_owned(),
                via: None
            }]
        );
        let floor = required_floor(root, &identity).unwrap();
        assert_eq!(
            floor.tokens[&ArtifactKind::Directive],
            vec!["001-arch".to_owned(), "002-style".to_owned()]
        );
    }

    #[test]
    fn extends_errors_and_missing_pack_are_refused_strictly() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let overlay = pack(root, "overlay");
        write(&overlay.join(ORG_CHARTER), "extends: absent\n");
        config(
            root,
            &format!(
                "charter_packs:\n  org:\n    packs:\n      - name: overlay\n        local_path: {}\n",
                overlay.display()
            ),
        );
        let error = org_charter_documents(root, true).unwrap_err();
        assert!(matches!(error, DoctrineError::OrgCharterExtends { .. }));
        // Lenient readers fall back to the declared pack alone.
        let docs = org_charter_documents(root, false).unwrap();
        assert_eq!(docs.len(), 1);

        config(
            root,
            "charter_packs:\n  org:\n    packs:\n      - name: ghost\n        local_path: /nonexistent/pack\n",
        );
        assert!(org_charter_documents(root, true).is_err());
        assert!(org_charter_documents(root, false).unwrap().is_empty());
    }

    #[test]
    fn org_expected_artifacts_and_requirement_kinds_use_the_last_match() {
        let directory = tempfile::tempdir().unwrap();
        let one = directory.path().join("one");
        let two = directory.path().join("two");
        write(
            &one.join("missions/software-dev/expected-artifacts.yaml"),
            "spec: [a]\n",
        );
        write(
            &two.join("missions/software-dev/expected-artifacts.yaml"),
            "spec: [b]\n",
        );
        write(
            &one.join("missions/software-dev/requirement-kinds.yaml"),
            "spec: mandatory\n",
        );
        let roots = vec![one, two];
        let expected = resolve_org_expected_artifacts(&roots, "software-dev")
            .unwrap()
            .unwrap();
        assert_eq!(
            expected["spec"][0].as_str(),
            Some("b"),
            "the last matching org root wins"
        );
        let kinds = resolve_org_requirement_kinds(&roots, "software-dev")
            .unwrap()
            .unwrap();
        assert_eq!(kinds["spec"].as_str(), Some("mandatory"));
        assert!(
            resolve_org_expected_artifacts(&roots, "research")
                .unwrap()
                .is_none()
        );
        // A mapping with unrelated keys is well-formed; a list root is not.
        write(
            &roots[1].join("missions/research/expected-artifacts.yaml"),
            "unrelated: true\n",
        );
        assert!(resolve_org_expected_artifacts(&roots, "research").is_ok());
        write(
            &roots[1].join("missions/research/expected-artifacts.yaml"),
            "- a list\n",
        );
        assert!(resolve_org_expected_artifacts(&roots, "research").is_err());
    }
}

#[cfg(test)]
mod packs_root_tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn packs_root_normalization_accepts_parent_and_built_in_leaf_spellings() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        // A packs root that is NOT discoverable as `<ancestor>/packs/built-in`:
        // the built-in tier must come from the explicit/normalized packs root,
        // so an unnormalized leaf spelling really fails here.
        let packs = root.join("packstore");
        let built_in = packs.join("built-in");
        let acme = packs.join("acme");
        write(
            &built_in.join("pack.yaml"),
            "name: built-in\npack_level: built-in\n",
        );
        write(
            &acme.join("org-charter.yaml"),
            "required_directives: [DIRECTIVE_001]\n",
        );
        write(
            &acme.join("directives/001-arch.directive.yaml"),
            "id: DIRECTIVE_001\n",
        );
        write(
            &root.join(".kittify/config.yaml"),
            &format!(
                "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
                acme.display()
            ),
        );

        // The pure normalization: the documented parent is unchanged, the
        // built-in leaf (which the CLI adapter also accepts) maps to it, and a
        // pack directory that merely ends in another name is left alone.
        assert_eq!(normalize_packs_root(&packs), packs);
        assert_eq!(normalize_packs_root(&built_in), packs);
        assert_eq!(normalize_packs_root(&acme), acme);

        let identity = IdentityIndex::from_roots(&built_in, std::slice::from_ref(&acme), None);
        // Both spellings resolve the same org registry and floor. Without the
        // shared normalization a leaf value made the built-in search fail,
        // which the lenient declared-root read silently turned into an EMPTY
        // floor — the org floor drop this API exists to prevent.
        let parent_roots = configured_org_roots_with_packs_root(root, Some(&packs)).unwrap();
        let leaf_roots = configured_org_roots_with_packs_root(root, Some(&built_in)).unwrap();
        assert_eq!(leaf_roots, parent_roots);
        assert_eq!(leaf_roots.len(), 1);
        assert_eq!(leaf_roots[0].0, "acme");

        let parent = required_floor_with_packs_root(root, &identity, Some(&packs)).unwrap();
        let leaf = required_floor_with_packs_root(root, &identity, Some(&built_in)).unwrap();
        assert_eq!(
            parent.tokens[&ArtifactKind::Directive],
            vec!["001-arch".to_owned()]
        );
        assert_eq!(leaf.tokens, parent.tokens);
        assert_eq!(
            read_org_required_tokens_with_packs_root(
                root,
                ArtifactKind::Directive,
                false,
                Some(&built_in)
            )
            .unwrap(),
            vec!["DIRECTIVE_001".to_owned()]
        );
    }
}
