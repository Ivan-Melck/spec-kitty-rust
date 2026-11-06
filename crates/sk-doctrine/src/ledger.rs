//! The activation ledger: vocabulary, typed load, deny guard and locked writes.
//!
//! Mirrors `src/charter/activation/activation_ledger.py` and
//! `src/kernel/activation_ledger_store.py`.
//!
//! `.kittify/charter/activation.yaml` is the single authority for activation.
//! A project without a ledger still reads its legacy allow keys from
//! `.kittify/config.yaml` (or the `charter:` pointer it names), but a legacy
//! surface can never carry a deactivation: a deny write is refused before
//! anything is written.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use sk_charter::ArtifactKind;
use sk_kernel::PythonString;
use sk_kernel::python_os_encoding::OsEncodingContext;
use sk_kernel::python_yaml::{self, Dialect};

use crate::activation_reporting;
use crate::error::DoctrineError;
use crate::identity::{IdentityFailure, IdentityIndex, IdentityLookup};
use crate::yaml::{PreparedWrite, YamlDocument};

/// The activation ledger, relative to the repository root.
pub const ACTIVATION_LEDGER_REL: &str = ".kittify/charter/activation.yaml";
/// `.kittify/config.yaml`, relative to the repository root.
pub const CONFIG_YAML_REL: &str = ".kittify/config.yaml";

const ALLOW_PREFIX: &str = "activated_";
pub(crate) const DENY_PREFIX: &str = "deactivated_";
const ACTIVATED_KINDS: &str = "activated_kinds";
const MISSION_TYPE_ACTIVATIONS: &str = "mission_type_activations";

/// The charter-activatable artifact kinds, in enum order.
pub fn activatable_kinds() -> Vec<ArtifactKind> {
    ArtifactKind::ALL
        .iter()
        .copied()
        .filter(|kind| kind.activatable())
        .collect()
}

/// What is in force for a kind while its activation key is absent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectiveWhenAbsent {
    /// Every available artifact is effective.
    All,
    /// Only the org-required set is effective (`skill`).
    Required,
}

impl EffectiveWhenAbsent {
    /// The rule for an artifact kind.
    pub fn for_kind(kind: ArtifactKind) -> Self {
        // Mirrors `artifact_kinds._REQUIRED_WHEN_ABSENT_KINDS` (currently `skill`).
        if kind == ArtifactKind::Skill {
            Self::Required
        } else {
            Self::All
        }
    }
}

fn require_activatable(kind: ArtifactKind) -> Result<ArtifactKind, DoctrineError> {
    if kind.activatable() {
        Ok(kind)
    } else {
        Err(DoctrineError::NoDenyList {
            key: kind.plural().to_owned(),
        })
    }
}

/// The ledger allow-list key of `kind` (`activated_<plural>`).
pub fn allow_key(kind: ArtifactKind) -> Result<String, DoctrineError> {
    Ok(format!(
        "{ALLOW_PREFIX}{}",
        require_activatable(kind)?.plural()
    ))
}

/// The ledger deny-list key of `kind` (`deactivated_<plural>`).
pub fn deny_key(kind: ArtifactKind) -> Result<String, DoctrineError> {
    Ok(format!(
        "{DENY_PREFIX}{}",
        require_activatable(kind)?.plural()
    ))
}

/// The two allow keys that are not artifact kinds.
pub fn non_kind_allow_keys() -> [&'static str; 2] {
    [ACTIVATED_KINDS, MISSION_TYPE_ACTIVATIONS]
}

/// Every ledger key, in the order new keys are appended on write.
pub fn ledger_keys() -> Vec<String> {
    let kinds = activatable_kinds();
    let mut keys: Vec<String> = non_kind_allow_keys()
        .into_iter()
        .map(str::to_owned)
        .collect();
    keys.extend(kinds.iter().filter_map(|kind| allow_key(*kind).ok()));
    keys.extend(kinds.iter().filter_map(|kind| deny_key(*kind).ok()));
    keys
}

fn ledger_key_set() -> BTreeSet<String> {
    ledger_keys().into_iter().collect()
}

/// Where activation is read from and written to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivationSource {
    /// The activation ledger exists (or the project is fresh).
    Ledger,
    /// The upgrade-only legacy surface.
    Legacy,
}

/// The resolver's answer: data only, the caller decides what to raise.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResolvedActivationSource {
    /// The resolved source.
    pub source: Option<ActivationSource>,
    /// The `charter:` pointer, when it is a string.
    pub pointer: Option<PathBuf>,
    /// Whether the pointer exists.
    pub pointer_exists: bool,
    /// Whether the ledger file exists.
    pub ledger_exists: bool,
    /// Legacy activation keys found beside an existing ledger.
    pub stray_legacy_keys: Vec<(PathBuf, Vec<String>)>,
    /// Legacy activation keys that have not been migrated.
    pub unmigrated_keys: Vec<(PathBuf, Vec<String>)>,
}

impl ResolvedActivationSource {
    /// The resolved source (always present after a successful resolve).
    pub fn source(&self) -> ActivationSource {
        self.source.unwrap_or(ActivationSource::Ledger)
    }

    /// True for an unmigrated project: legacy keys to move, no ledger.
    pub fn unmigrated(&self) -> bool {
        !self.unmigrated_keys.is_empty()
    }
}

/// A ledger token: the exact Python code-point string, or its checked scalar view.
///
/// The ledger model, the builder and the deny algebra are declared **once** over
/// this trait; [`String`] instantiates the crate's scalar capability view and
/// [`PythonString`] instantiates the exact Source data. A token with no scalar
/// projection (a lone surrogate, which Source's `str` holds) is never dropped or
/// replaced on the exact path, and its scalar projection is an explicit checked
/// refusal ([`LedgerData::to_scalar_view`]).
pub trait LedgerToken: Clone + std::fmt::Debug + Eq + Ord {
    /// The token for a scalar (always-representable) string.
    fn from_text(text: &str) -> Self;
    /// The token for an exact code-point string, when this instantiation can hold it.
    fn from_exact(token: &PythonString) -> Option<Self>;
    /// The scalar projection of this token, when it has one.
    fn to_text(&self) -> Option<String>;
    /// Exact membership test against a scalar spelling.
    fn matches_text(&self, text: &str) -> bool;
    /// Exact membership test against a code-point spelling.
    fn matches_exact(&self, token: &PythonString) -> bool;
}

impl LedgerToken for String {
    fn from_text(text: &str) -> Self {
        text.to_owned()
    }

    fn from_exact(token: &PythonString) -> Option<Self> {
        token.to_scalar_string().ok()
    }

    fn to_text(&self) -> Option<String> {
        Some(self.clone())
    }

    fn matches_text(&self, text: &str) -> bool {
        self == text
    }

    fn matches_exact(&self, token: &PythonString) -> bool {
        token.to_scalar_string().is_ok_and(|text| text == *self)
    }
}

impl LedgerToken for PythonString {
    fn from_text(text: &str) -> Self {
        PythonString::from_scalars(text)
    }

    fn from_exact(token: &PythonString) -> Option<Self> {
        Some(token.clone())
    }

    fn to_text(&self) -> Option<String> {
        self.to_scalar_string().ok()
    }

    fn matches_text(&self, text: &str) -> bool {
        *self == PythonString::from_scalars(text)
    }

    fn matches_exact(&self, token: &PythonString) -> bool {
        self == token
    }
}

/// The typed activation state of one project, over one token instantiation.
///
/// [`ActivationLedger`] is the scalar capability view; [`ExactActivationLedger`]
/// is the exact Source data. They are the SAME declaration and the same builder,
/// so no field, key rule or three-state rule can drift between them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerData<T = String> {
    /// The backing file.
    pub path: PathBuf,
    /// Where the state was read from.
    pub source: ActivationSource,
    /// Whether the backing file exists.
    pub exists: bool,
    /// `activated_kinds`, or `None` when absent (all kinds active).
    pub activated_kinds: Option<Vec<T>>,
    /// `mission_type_activations`, or `None` when absent (nothing active).
    pub mission_type_activations: Option<Vec<T>>,
    /// Per-kind allow state; `None` is the three-state "absent".
    pub allow: BTreeMap<ArtifactKind, Option<Vec<T>>>,
    /// Per-kind deny list (empty when nothing is denied).
    pub deny: BTreeMap<ArtifactKind, Vec<T>>,
    /// Unknown `deactivated_<kind>` keys, kept verbatim on write.
    pub unknown_deny_keys: Vec<T>,
    /// The unknown deny keys' values, so a round-trip never drops them.
    pub unknown_deny_values: Vec<(T, Option<Vec<T>>)>,
    /// Legacy keys found beside the ledger (ignored; the ledger wins).
    pub stray_legacy_keys: Vec<(PathBuf, Vec<String>)>,
    /// Legacy `deactivated_<kind>` keys that were reported but not applied.
    pub ignored_legacy_deny_keys: Vec<T>,
}

/// The scalar capability view of the activation ledger.
pub type ActivationLedger = LedgerData<String>;
/// The exact activation ledger: Source's `str` code points, lone surrogates included.
pub type ExactActivationLedger = LedgerData<PythonString>;

impl<T> Default for LedgerData<T> {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            source: ActivationSource::Ledger,
            exists: false,
            activated_kinds: None,
            mission_type_activations: None,
            allow: BTreeMap::new(),
            deny: BTreeMap::new(),
            unknown_deny_keys: Vec::new(),
            unknown_deny_values: Vec::new(),
            stray_legacy_keys: Vec::new(),
            ignored_legacy_deny_keys: Vec::new(),
        }
    }
}

impl<T: LedgerToken> LedgerData<T> {
    /// The allow state of `kind`.
    pub fn allow_for(&self, kind: ArtifactKind) -> Option<&[T]> {
        self.allow.get(&kind).and_then(Option::as_deref)
    }

    /// The deny list of `kind`.
    pub fn deny_for(&self, kind: ArtifactKind) -> &[T] {
        self.deny.get(&kind).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The three-state activated-kind set: `None` means every kind.
    pub fn activated_kinds_set(&self) -> Option<BTreeSet<T>> {
        self.activated_kinds
            .as_ref()
            .map(|kinds| kinds.iter().cloned().collect())
    }

    /// The mission types in force.
    pub fn mission_types(&self) -> BTreeSet<T> {
        self.mission_type_activations
            .as_ref()
            .map(|items| items.iter().cloned().collect())
            .unwrap_or_default()
    }
}

impl LedgerData<String> {
    /// The present ledger keys as a plain mapping, in ledger-key order.
    ///
    /// Unknown `deactivated_<kind>` keys are kept verbatim last, so a write
    /// round-trips everything it did not understand instead of dropping it.
    pub fn raw(&self) -> BTreeMap<String, Option<Vec<String>>> {
        let mut raw = BTreeMap::new();
        raw.insert(ACTIVATED_KINDS.to_owned(), self.activated_kinds.clone());
        raw.insert(
            MISSION_TYPE_ACTIVATIONS.to_owned(),
            self.mission_type_activations.clone(),
        );
        for kind in activatable_kinds() {
            if let Ok(key) = allow_key(kind)
                && let Some(value) = self.allow.get(&kind).cloned().flatten()
            {
                raw.insert(key, Some(value));
            }
        }
        for kind in activatable_kinds() {
            if let Ok(key) = deny_key(kind)
                && let Some(value) = self.deny.get(&kind).filter(|ids| !ids.is_empty())
            {
                raw.insert(key, Some(value.clone()));
            }
        }
        for (key, value) in &self.unknown_deny_values {
            raw.insert(key.clone(), value.clone());
        }
        raw
    }
}

/// A token of an exact ledger whose code-point sequence has no scalar projection.
///
/// This is a **Native capability boundary**, not Source invalidity: Source holds
/// the lone surrogate happily. A caller that needs scalar strings must handle
/// this refusal instead of receiving a dropped or substituted id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScalarProjectionUnavailable {
    /// The ledger field (`deactivated_directives`, ...).
    pub key: String,
    /// The offending Python code point.
    pub codepoint: u32,
}

impl std::fmt::Display for ScalarProjectionUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "activation key {} carries code point U+{:04X}, which has no scalar (UTF-8) projection; \
             this is a Native scalar-view limit, not an invalid Source ledger",
            self.key, self.codepoint
        )
    }
}

impl std::error::Error for ScalarProjectionUnavailable {}

impl ExactActivationLedger {
    /// The checked scalar projection of this exact ledger.
    ///
    /// Every token must have a scalar projection; the first one that does not is
    /// an explicit [`ScalarProjectionUnavailable`], never a dropped, replaced or
    /// re-rendered id.
    ///
    /// # Errors
    ///
    /// The first token without a scalar projection.
    pub fn to_scalar_view(&self) -> Result<ActivationLedger, ActivationReadError> {
        let scalar = |key: &str, token: &PythonString| -> Result<String, ActivationReadError> {
            token.to_scalar_string().map_err(|codepoint| {
                ActivationReadError::ScalarProjection(ScalarProjectionUnavailable {
                    key: key.to_owned(),
                    codepoint,
                })
            })
        };
        let mut projected = ActivationLedger {
            path: self.path.clone(),
            source: self.source,
            exists: self.exists,
            activated_kinds: None,
            mission_type_activations: None,
            allow: BTreeMap::new(),
            deny: BTreeMap::new(),
            unknown_deny_keys: self
                .unknown_deny_keys
                .iter()
                .map(|key| scalar("<unknown deny key>", key))
                .collect::<Result<_, _>>()?,
            unknown_deny_values: Vec::new(),
            stray_legacy_keys: self.stray_legacy_keys.clone(),
            ignored_legacy_deny_keys: self
                .ignored_legacy_deny_keys
                .iter()
                .map(|key| scalar("<ignored legacy key>", key))
                .collect::<Result<_, _>>()?,
        };
        projected.activated_kinds = self
            .activated_kinds
            .as_ref()
            .map(|ids| ids.iter().map(|id| scalar(ACTIVATED_KINDS, id)).collect())
            .transpose()?;
        projected.mission_type_activations = self
            .mission_type_activations
            .as_ref()
            .map(|ids| {
                ids.iter()
                    .map(|id| scalar(MISSION_TYPE_ACTIVATIONS, id))
                    .collect()
            })
            .transpose()?;
        for (kind, ids) in &self.allow {
            let key = allow_key(*kind).map_err(ActivationReadError::Doctrine)?;
            let value = ids
                .as_ref()
                .map(|ids| ids.iter().map(|id| scalar(&key, id)).collect())
                .transpose()?;
            projected.allow.insert(*kind, value);
        }
        for (kind, ids) in &self.deny {
            let key = deny_key(*kind).map_err(ActivationReadError::Doctrine)?;
            let value = ids
                .iter()
                .map(|id| scalar(&key, id))
                .collect::<Result<Vec<_>, _>>()?;
            projected.deny.insert(*kind, value);
        }
        for (key, ids) in &self.unknown_deny_values {
            let value = ids
                .as_ref()
                .map(|ids| ids.iter().map(|id| scalar(&key.repr(), id)).collect())
                .transpose()?;
            projected
                .unknown_deny_values
                .push((scalar("<unknown deny key>", key)?, value));
        }
        Ok(projected)
    }
}

/// The activation ledger path for a repository root.
pub fn activation_ledger_path(repo_root: &Path) -> PathBuf {
    repo_root.join(ACTIVATION_LEDGER_REL)
}

fn read_mapping(path: &Path, display: &Path) -> Result<YamlDocument, DoctrineError> {
    let document = YamlDocument::open(path)?;
    if !matches!(
        document.value(),
        yaml_edit::YamlValue::Mapping(_) | yaml_edit::YamlValue::Scalar(_)
    ) {
        return Err(DoctrineError::YamlRootNotMapping {
            path: display.to_path_buf(),
        });
    }
    Ok(document)
}

fn config_document(repo_root: &Path) -> Result<YamlDocument, DoctrineError> {
    let path = repo_root.join(CONFIG_YAML_REL);
    if !path.exists() {
        return Ok(YamlDocument::empty(&path));
    }
    read_mapping(&path, &path)
}

fn pointer_path(repo_root: &Path, pointer: Option<&str>) -> (Option<PathBuf>, bool) {
    let Some(pointer) = pointer else {
        return (None, false);
    };
    let candidate = PathBuf::from(pointer);
    let path = if candidate.is_absolute() {
        candidate
    } else {
        repo_root.join(candidate)
    };
    let exists = path.exists();
    (Some(path), exists)
}

fn legacy_allow_keys() -> BTreeSet<String> {
    let mut keys: BTreeSet<String> = non_kind_allow_keys()
        .into_iter()
        .map(str::to_owned)
        .collect();
    keys.extend(
        activatable_kinds()
            .into_iter()
            .filter_map(|kind| allow_key(kind).ok()),
    );
    keys
}

fn legacy_keys_in(probe: &ProbeSnapshot, allowed: &BTreeSet<String>) -> Vec<String> {
    let mut keys: Vec<String> = probe
        .keys
        .iter()
        .filter(|key| allowed.contains(*key))
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// Resolve where this repository's activation lives.
pub fn resolve_activation_source(
    repo_root: &Path,
) -> Result<ResolvedActivationSource, DoctrineError> {
    // The Source activation seam freshly validates config, then its kernel
    // resolver uses the shared cached key/string snapshot (including pointer).
    config_document(repo_root)?;
    let config_path = repo_root.join(CONFIG_YAML_REL);
    let config = probe_snapshot(&config_path)?;
    let (pointer, pointer_exists) =
        pointer_path(repo_root, config.strings.get("charter").map(String::as_str));
    let ledger_path = activation_ledger_path(repo_root);
    let ledger_exists = sk_kernel::paths::source_path_is_file(&ledger_path)
        .map_err(|error| DoctrineError::io("stat activation ledger", &ledger_path, error))?;
    let ledger_keys = ledger_key_set();
    let mut found: Vec<(PathBuf, Vec<String>)> = Vec::new();
    let config_keys = legacy_keys_in(&config, &ledger_keys);
    if !config_keys.is_empty() {
        found.push((config_path, config_keys));
    }
    if let (Some(pointer), true) = (&pointer, pointer_exists) {
        let pointed = probe_snapshot(pointer)?;
        let keys = legacy_keys_in(&pointed, &ledger_keys);
        if !keys.is_empty() {
            found.push((pointer.clone(), keys));
        }
    }

    if ledger_exists {
        return Ok(ResolvedActivationSource {
            source: Some(ActivationSource::Ledger),
            pointer,
            pointer_exists,
            ledger_exists: true,
            stray_legacy_keys: found,
            unmigrated_keys: Vec::new(),
        });
    }
    if pointer.is_some() && !pointer_exists {
        return Ok(ResolvedActivationSource {
            source: Some(ActivationSource::Legacy),
            pointer,
            pointer_exists,
            ledger_exists: false,
            stray_legacy_keys: Vec::new(),
            unmigrated_keys: found,
        });
    }
    if !found.is_empty() {
        return Ok(ResolvedActivationSource {
            source: Some(ActivationSource::Legacy),
            pointer,
            pointer_exists,
            ledger_exists: false,
            stray_legacy_keys: Vec::new(),
            unmigrated_keys: found,
        });
    }
    Ok(ResolvedActivationSource {
        source: Some(ActivationSource::Ledger),
        pointer,
        pointer_exists,
        ledger_exists: false,
        stray_legacy_keys: Vec::new(),
        unmigrated_keys: Vec::new(),
    })
}

/// The typed read surface the ONE builder consumes: the existing `YamlDocument`
/// scalar view, or the canonical Kernel Python-YAML graph (exact).
///
/// A view only answers "what are this mapping's top-level keys?" and "what list
/// of string ids does this key hold?" — every schema, three-state and error rule
/// lives once in [`build_ledger`].
pub(crate) enum LedgerKey {
    Text(PythonString),
    Other(String),
}

pub(crate) trait LedgerReadView {
    fn keys(&self) -> Result<Vec<LedgerKey>, DoctrineError>;
    fn list(
        &self,
        key: &PythonString,
        strict_ids: bool,
    ) -> Result<Option<Vec<PythonString>>, DoctrineError>;
}

/// The pre-existing `YamlDocument` read, as a checked **scalar** view.
struct YamlView<'a>(&'a YamlDocument);

impl LedgerReadView for YamlView<'_> {
    fn keys(&self) -> Result<Vec<LedgerKey>, DoctrineError> {
        Ok(self
            .0
            .keys()
            .iter()
            .map(|key| LedgerKey::Text(PythonString::from_scalars(key)))
            .collect())
    }

    fn list(
        &self,
        key: &PythonString,
        _strict_ids: bool,
    ) -> Result<Option<Vec<PythonString>>, DoctrineError> {
        let key = key
            .to_scalar_string()
            .expect("a scalar view only requests scalar keys");
        Ok(self.0.get_string_list(&key)?.map(|ids| {
            ids.iter()
                .map(|id| PythonString::from_scalars(id))
                .collect()
        }))
    }
}

/// A single view over an existing filter (the legacy allow-key projection).
struct FilteredView<'a> {
    inner: &'a dyn LedgerReadView,
    allowed: &'a BTreeSet<String>,
}

impl LedgerReadView for FilteredView<'_> {
    fn keys(&self) -> Result<Vec<LedgerKey>, DoctrineError> {
        Ok(self
            .inner
            .keys()?
            .into_iter()
            .filter(|key| match key {
                LedgerKey::Text(text) => text
                    .to_scalar_string()
                    .is_ok_and(|text| self.allowed.contains(&text)),
                LedgerKey::Other(_) => false,
            })
            .collect())
    }

    fn list(
        &self,
        key: &PythonString,
        strict_ids: bool,
    ) -> Result<Option<Vec<PythonString>>, DoctrineError> {
        if key
            .to_scalar_string()
            .is_ok_and(|text| self.allowed.contains(&text))
        {
            self.inner.list(key, strict_ids)
        } else {
            Ok(None)
        }
    }
}

/// No document at all (an absent ledger).
struct EmptyView;

impl LedgerReadView for EmptyView {
    fn keys(&self) -> Result<Vec<LedgerKey>, DoctrineError> {
        Ok(Vec::new())
    }

    fn list(
        &self,
        _key: &PythonString,
        _strict_ids: bool,
    ) -> Result<Option<Vec<PythonString>>, DoctrineError> {
        Ok(None)
    }
}

/// The canonical Kernel Python-YAML graph, as the **exact** view.
///
/// This is the Source data authority: values are the loader's `str` code points
/// (lone surrogates, NUL, PUA and noncharacters included), so no token has to
/// pass through a UTF-8 `String` on this path.
struct GraphView<'a> {
    document: &'a python_yaml::GraphDocument,
    path: PathBuf,
}

impl GraphView<'_> {
    fn pairs(&self) -> Option<&[(python_yaml::NodeId, python_yaml::NodeId)]> {
        let root = self.document.root();
        root.as_mapping().or_else(|| root.as_ordered_dict())
    }

    fn key(&self, id: python_yaml::NodeId) -> LedgerKey {
        match self.document.view(id).as_str() {
            Some(text) => LedgerKey::Text(text.clone()),
            None => LedgerKey::Other(self.document.graph().repr(id)),
        }
    }
}

impl LedgerReadView for GraphView<'_> {
    fn keys(&self) -> Result<Vec<LedgerKey>, DoctrineError> {
        Ok(self
            .pairs()
            .map(|pairs| pairs.iter().map(|(key, _)| self.key(*key)).collect())
            .unwrap_or_default())
    }

    fn list(
        &self,
        key: &PythonString,
        strict_ids: bool,
    ) -> Result<Option<Vec<PythonString>>, DoctrineError> {
        let Some((_, value)) = self.pairs().and_then(|pairs| {
            pairs
                .iter()
                .find(|(key_id, _)| self.document.view(*key_id).as_str() == Some(key))
        }) else {
            return Ok(None);
        };
        let node = self.document.view(*value);
        if node.is_null() {
            return Ok(None);
        }
        let Some(items) = node.as_sequence() else {
            return Err(DoctrineError::InvalidLedgerValue {
                path: self.path.clone(),
                key: key.to_scalar_string().unwrap_or_else(|_| key.repr()),
                message: node.python_type_name().to_owned(),
            });
        };
        let mut ids = Vec::with_capacity(items.len());
        for item in items {
            let item_node = self.document.view(*item);
            if !strict_ids {
                ids.push(item_node.python_str());
            } else if let Some(text) = item_node.as_str() {
                ids.push(text.clone());
            } else {
                return Err(DoctrineError::InvalidLedgerValue {
                    path: self.path.clone(),
                    key: key.to_scalar_string().unwrap_or_else(|_| key.repr()),
                    message: format!(
                        "item {} is not a string id",
                        self.document.graph().repr(*item)
                    ),
                });
            }
        }
        Ok(Some(ids))
    }
}

/// Project one read list onto the model's token instantiation.
fn tokens_for<T: LedgerToken>(
    path: &Path,
    key: &str,
    exact: Option<Vec<PythonString>>,
) -> Result<Option<Vec<T>>, DoctrineError> {
    let Some(items) = exact else {
        return Ok(None);
    };
    let mut tokens = Vec::with_capacity(items.len());
    for item in &items {
        match T::from_exact(item) {
            Some(token) => tokens.push(token),
            None => {
                return Err(DoctrineError::InvalidLedgerValue {
                    path: path.to_path_buf(),
                    key: key.to_owned(),
                    message: format!(
                        "code point {} has no scalar (UTF-8) projection; this is a Native \
                         scalar-view limit, not an invalid Source ledger",
                        item.codepoints()
                            .iter()
                            .find(|code| char::from_u32(**code).is_none())
                            .map(|code| format!("U+{code:04X}"))
                            .unwrap_or_else(|| "<unknown>".to_owned())
                    ),
                });
            }
        }
    }
    Ok(Some(tokens))
}

fn key_is_deny(key: &PythonString) -> bool {
    key.codepoints()
        .iter()
        .copied()
        .take(DENY_PREFIX.len())
        .eq(DENY_PREFIX.chars().map(u32::from))
}

fn key_token<T: LedgerToken>(path: &Path, key: &PythonString) -> Result<T, DoctrineError> {
    let projected = tokens_for::<T>(path, "<ledger key>", Some(vec![key.clone()]))?
        .expect("a present list remains present");
    Ok(projected
        .into_iter()
        .next()
        .expect("one key projects to one token"))
}

/// The ONE ledger builder: schema, three-state rule and L1/L2 validation.
fn build_ledger<T: LedgerToken>(
    path: PathBuf,
    source: ActivationSource,
    exists: bool,
    view: &dyn LedgerReadView,
    strict: bool,
) -> Result<LedgerData<T>, DoctrineError> {
    // Source reads EVERY known field in LEDGER_KEYS order before validating
    // unknown keys, then validates unknown deny values only after all keys.
    let read = |key: &str| view.list(&PythonString::from_scalars(key), strict);
    let activated_kinds = tokens_for::<T>(&path, ACTIVATED_KINDS, read(ACTIVATED_KINDS)?)?;
    let mission_type_activations = tokens_for::<T>(
        &path,
        MISSION_TYPE_ACTIVATIONS,
        read(MISSION_TYPE_ACTIVATIONS)?,
    )?;
    let mut allow = BTreeMap::new();
    for kind in activatable_kinds() {
        let key = allow_key(kind)?;
        allow.insert(kind, tokens_for::<T>(&path, &key, read(&key)?)?);
    }
    let mut deny = BTreeMap::new();
    for kind in activatable_kinds() {
        let key = deny_key(kind)?;
        deny.insert(
            kind,
            tokens_for::<T>(&path, &key, read(&key)?)?.unwrap_or_default(),
        );
    }
    let known = ledger_key_set();
    let keys = view.keys()?;
    let mut unknown = Vec::new();
    if strict {
        for key in &keys {
            match key {
                LedgerKey::Text(text)
                    if text
                        .to_scalar_string()
                        .is_ok_and(|text| known.contains(&text)) => {}
                LedgerKey::Text(text) if key_is_deny(text) => unknown.push(text.clone()),
                other => {
                    return Err(DoctrineError::UnknownLedgerKey {
                        path,
                        key: match other {
                            LedgerKey::Text(text) => {
                                text.to_scalar_string().unwrap_or_else(|_| text.repr())
                            }
                            LedgerKey::Other(repr) => repr.clone(),
                        },
                        allowed: ledger_keys().join(", "),
                    });
                }
            }
        }
    }
    let mut unknown_deny_keys = Vec::new();
    let mut unknown_deny_values = Vec::new();
    for key in unknown {
        let token = key_token::<T>(&path, &key)?;
        unknown_deny_keys.push(token.clone());
        if let Some(values) = tokens_for::<T>(&path, &key.repr(), view.list(&key, true)?)? {
            unknown_deny_values.push((token, Some(values)));
        }
    }
    let ignored_legacy_deny_keys = if source == ActivationSource::Legacy {
        let mut ignored = Vec::new();
        for key in keys {
            if let LedgerKey::Text(text) = key
                && key_is_deny(&text)
            {
                ignored.push(key_token::<T>(&path, &text)?);
            }
        }
        ignored.sort();
        ignored
    } else {
        Vec::new()
    };
    Ok(LedgerData {
        path,
        source,
        exists,
        activated_kinds,
        mission_type_activations,
        allow,
        deny,
        unknown_deny_keys,
        unknown_deny_values,
        stray_legacy_keys: Vec::new(),
        ignored_legacy_deny_keys,
    })
}

/// The legacy surface the resolver names, without reading it.
///
/// Source's `_legacy_path`: the pointed `charter.yaml` when the pointer exists,
/// otherwise `config.yaml`. A dangling pointer is the same refusal the typed
/// read raises, so the probe never answers "no deny key" for an unreadable
/// activation source.
fn legacy_probe_path(
    repo_root: &Path,
    resolved: &ResolvedActivationSource,
) -> Result<PathBuf, DoctrineError> {
    match &resolved.pointer {
        Some(pointer) if resolved.pointer_exists => Ok(pointer.clone()),
        Some(pointer) => Err(DoctrineError::Yaml {
            path: repo_root.join(CONFIG_YAML_REL),
            message: format!(
                ".kittify/config.yaml 'charter:' pointer names {}, which does not exist",
                pointer.display()
            ),
        }),
        None => Ok(repo_root.join(CONFIG_YAML_REL)),
    }
}

/// The cache key of one probed file: resolved path, mtime and size.
type ProbeKey = (PathBuf, i128, u64);

#[derive(Clone, Default)]
struct ProbeSnapshot {
    keys: BTreeSet<String>,
    strings: BTreeMap<String, String>,
}

fn probe_cache() -> &'static Mutex<BTreeMap<ProbeKey, ProbeSnapshot>> {
    static CACHE: OnceLock<Mutex<BTreeMap<ProbeKey, ProbeSnapshot>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn probe_key(path: &Path, metadata: &std::fs::Metadata) -> Result<ProbeKey, DoctrineError> {
    // Source uses signed integer st_mtime_ns, including pre-epoch timestamps.
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let modified = metadata
        .modified()
        .map_err(|error| DoctrineError::io("read YAML probe mtime", path, error))?;
    let (duration, sign) = match modified.duration_since(UNIX_EPOCH) {
        Ok(duration) => (duration, 1_i128),
        Err(before) => (before.duration(), -1_i128),
    };
    let nanos = (i128::from(duration.as_secs()) * 1_000_000_000
        + i128::from(duration.subsec_nanos()))
        * sign;
    Ok((resolved, nanos, metadata.len()))
}

/// The top-level keys of the YAML mapping at `path` (Source `top_level_keys`).
///
/// Empty unless `path` is a regular file, and served from a per-process cache
/// keyed on the resolved path, mtime and size (NFR-003), so asking again for an
/// unchanged file costs a stat. The one lossless parse authority is reused: a
/// malformed document is its existing typed refusal, never "no deny key".
fn top_level_keys(path: &Path) -> Result<BTreeSet<String>, DoctrineError> {
    Ok(probe_snapshot(path)?.keys)
}

fn probe_snapshot(path: &Path) -> Result<ProbeSnapshot, DoctrineError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ProbeSnapshot::default());
        }
        Err(source) => return Err(DoctrineError::io("probe YAML keys", path, source)),
    };
    if !metadata.is_file() {
        return Ok(ProbeSnapshot::default());
    }
    let key = probe_key(path, &metadata)?;
    if let Ok(cache) = probe_cache().lock()
        && let Some(keys) = cache.get(&key)
    {
        return Ok(keys.clone());
    }
    let document = YamlDocument::open(path)?;
    let keys: BTreeSet<String> = document.keys().into_iter().collect();
    let strings = keys
        .iter()
        .filter_map(|key| {
            document
                .get_str(key)
                .map(|value| (key.clone(), value.to_owned()))
        })
        .collect();
    let snapshot = ProbeSnapshot { keys, strings };
    if let Ok(mut cache) = probe_cache().lock() {
        cache.insert(key, snapshot.clone());
    }
    Ok(snapshot)
}

/// Whether `repo_root`'s activation source can carry a deny list, without a typed read.
///
/// Source `ledger_may_hold_deny`: a `LEDGER` source answers `true` (it is small
/// and simply read); a `LEGACY` surface is answered from the cached top-level
/// key probe, so a legacy file with no `deactivated_<kind>` key never pays the
/// full typed load, identity index or required-floor read (NFR-003). A legacy
/// deny key answers `true`, so the full read reports it (it is never applied).
///
/// # Errors
///
/// The resolver's read failures and the probe's parse/IO refusal; never a
/// healthy `false` for an unreadable activation surface.
pub fn ledger_may_hold_deny(repo_root: &Path) -> Result<bool, DoctrineError> {
    let resolved = resolve_activation_source(repo_root)?;
    if resolved.source() != ActivationSource::Legacy {
        return Ok(true);
    }
    let keys = top_level_keys(&legacy_probe_path(repo_root, &resolved)?)?;
    Ok(keys.iter().any(|key| key.starts_with(DENY_PREFIX)))
}

/// Options of the Source activation-ledger read.
///
/// Source's one keyword is `report_stray`: the ordinary command read reports
/// stray legacy keys beside a ledger (FR-020) and a doctor/inspection caller
/// passes `false` and reads [`LedgerData::stray_legacy_keys`] as data. The
/// legacy deny report follows the same switch (it is part of the same read).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LedgerReadOptions {
    /// Whether a read stage delivers its Source warning.
    pub report_stray: bool,
}

impl Default for LedgerReadOptions {
    fn default() -> Self {
        Self::REPORT
    }
}

impl LedgerReadOptions {
    /// Source's ordinary command read (`report_stray=True`).
    pub const REPORT: Self = Self { report_stray: true };
    /// Source's inspection read (`report_stray=False`): doctor findings, the
    /// status collectors and internal reads that must not print.
    pub const INSPECT: Self = Self {
        report_stray: false,
    };
}

/// Map one reporting failure onto this reader's real-cause error.
///
/// Source's `print` raises the stream's own exception out of the read; Native
/// keeps the real cause (the stream failure or the path-spelling refusal) as a
/// typed IO error instead of a healthy result or an invented activation class.
fn reporting_error(
    repo_root: &Path,
    error: activation_reporting::ActivationReportError,
) -> DoctrineError {
    DoctrineError::Io {
        action: "deliver the Source activation warning on stderr",
        path: repo_root.to_path_buf(),
        source: io::Error::other(error),
    }
}

/// The legacy surface's `deactivated_<kind>` report, at Source's stage.
fn report_legacy_deny_keys(
    repo_root: &Path,
    path: &Path,
    keys: impl IntoIterator<Item = PythonString>,
    codec: &OsEncodingContext,
) -> Result<(), DoctrineError> {
    activation_reporting::report_legacy_deny_keys_to_stderr(
        path,
        activation_reporting::legacy_deny_keys(keys),
        codec,
    )
    .map_err(|error| reporting_error(repo_root, error))
}

/// The stray-keys report, at Source's stage (beside a ledger).
fn report_stray_legacy_keys(
    repo_root: &Path,
    stray: &[(PathBuf, Vec<String>)],
    codec: &OsEncodingContext,
) -> Result<(), DoctrineError> {
    activation_reporting::report_stray_legacy_keys_to_stderr(stray, codec)
        .map_err(|error| reporting_error(repo_root, error))
}

/// The raw top-level text keys of one exact graph document (non-text keys are
/// not stringified, exactly like Source's `isinstance(key, str)`).
fn graph_text_keys(
    path: &Path,
    document: &python_yaml::GraphDocument,
) -> Result<Vec<PythonString>, DoctrineError> {
    let view = GraphView {
        document,
        path: path.to_path_buf(),
    };
    let mut keys = Vec::new();
    for key in view.keys()? {
        if let LedgerKey::Text(text) = key {
            keys.push(text);
        }
    }
    Ok(keys)
}

/// Load the typed activation state for `repo_root` — the **scalar capability view**.
///
/// This is an explicit checked view of ONE read truth: it calls the canonical
/// exact reader ([`load_activation_ledger_exact`]) and then the existing
/// [`ExactActivationLedger::to_scalar_view`]. It therefore inherits the Source
/// `str()` coercion of a non-strict legacy list (`[true, 123, null, ...]` becomes
/// `"True"`, `"123"`, `"None"`, ...) and the modern strict string-id rule, and
/// a token with no scalar projection is an explicit checked refusal. There is no
/// second parser, coercer or schema on this path, and the wrapper performs no
/// extra read, report or resolver pass.
///
/// Both Source warning stages are delivered by default ([`LedgerReadOptions::REPORT`]).
///
/// # Errors
///
/// The exact reader's typed refusal (an existing [`DoctrineError`] variant,
/// unwrapped unchanged), or the one boxed typed [`ActivationReadError`] cause for
/// a checked projection that has no scalar form or a runtime constructor abort.
pub fn load_activation_ledger(repo_root: &Path) -> Result<ActivationLedger, DoctrineError> {
    load_activation_ledger_with(repo_root, LedgerReadOptions::REPORT)
}

/// The Source inspection read: both warning stages are suppressed, the data stays.
pub fn load_activation_ledger_inspection(
    repo_root: &Path,
) -> Result<ActivationLedger, DoctrineError> {
    load_activation_ledger_with(repo_root, LedgerReadOptions::INSPECT)
}

/// Load the scalar capability view with Source's `report_stray` option.
///
/// # Errors
///
/// See [`load_activation_ledger`].
pub fn load_activation_ledger_with(
    repo_root: &Path,
    options: LedgerReadOptions,
) -> Result<ActivationLedger, DoctrineError> {
    let exact = load_activation_ledger_exact_with(repo_root, options).map_err(scalar_read_error)?;
    exact.to_scalar_view().map_err(scalar_read_error)
}

/// Map one exact-read failure onto the scalar API's error type.
///
/// A failure the exact reader already classified as a [`DoctrineError`] is the
/// SAME existing variant (never rewrapped, recursed or stringified). The
/// remaining typed causes — a checked projection with no scalar form, or a
/// runtime constructor abort — travel as the one boxed [`ActivationReadError`].
fn scalar_read_error(error: ActivationReadError) -> DoctrineError {
    match error {
        ActivationReadError::Doctrine(error) => error,
        other => DoctrineError::ActivationRead {
            source: Box::new(other),
        },
    }
}

/// A typed refusal of the exact activation read, with the real cause kept.
///
/// The arms are deliberately distinct: a `Doctrine` failure is the existing typed
/// refusal of the resolver / config / ledger-schema / floor authorities, `Aborted`
/// is a Python exception the pinned Source loader does **not** catch (an invalid
/// `!!timestamp`/`!!int` constructor in the ledger document, or an identity scan
/// whose artifact construction raises), and `Native` is a Native process-capability
/// failure (an unusable `PYTHONHASHSEED`, an unavailable default stream backend)
/// that has no Source exception class at all. Source's own read raises the first
/// two; it never becomes a healthy empty deny set and is never labelled with a
/// fabricated Python class.
#[derive(Debug)]
pub enum ActivationReadError {
    /// A typed refusal of an existing doctrine authority.
    Doctrine(DoctrineError),
    /// An exact token with no scalar (UTF-8) projection, asked of the scalar view.
    ScalarProjection(ScalarProjectionUnavailable),
    /// A Python runtime exception outside the loader's `except` clause.
    Aborted {
        /// The read phase that aborted (`activation ledger`, `legacy charter`, `identity scan`).
        phase: String,
        /// The file the phase was reading, or the repository root for a scan.
        path: PathBuf,
        /// The Python exception class name.
        python: String,
        /// Its message.
        message: String,
    },
    /// A typed Native capability/start-up failure (never a Source exception class).
    ///
    /// The activation-set-order secret and the default warning transport are
    /// process-level capabilities CPython resolves before user code. An unusable
    /// `PYTHONHASHSEED` or an unavailable stream backend is therefore carried here as
    /// [`crate::offering::NativeOfferingFailure`], not as a fabricated
    /// `Python` class name; the original Kernel cause and phase/path are retained.
    Native(crate::offering::NativeOfferingFailure),
}

impl From<DoctrineError> for ActivationReadError {
    fn from(error: DoctrineError) -> Self {
        Self::Doctrine(error)
    }
}

impl From<ScalarProjectionUnavailable> for ActivationReadError {
    fn from(error: ScalarProjectionUnavailable) -> Self {
        Self::ScalarProjection(error)
    }
}

impl std::fmt::Display for ActivationReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Doctrine(error) => std::fmt::Display::fmt(error, formatter),
            Self::ScalarProjection(error) => std::fmt::Display::fmt(error, formatter),
            Self::Aborted {
                phase,
                path,
                python,
                message,
            } => write!(
                formatter,
                "{phase} read aborted by {}: {python}: {message}",
                path.display()
            ),
            Self::Native(native) => std::fmt::Display::fmt(native, formatter),
        }
    }
}

impl std::error::Error for ActivationReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Doctrine(error) => Some(error),
            Self::ScalarProjection(error) => Some(error),
            Self::Aborted { .. } => None,
            Self::Native(native) => Some(native),
        }
    }
}

/// Map one canonical-reader failure onto Source's own outcome classes: a
/// `yaml.YAMLError` / transport refusal is the format error the ledger read wraps,
/// while a runtime exception escapes the load in Source and is kept as an abort.
fn map_document_error(path: &Path, error: &python_yaml::YamlError) -> ActivationReadError {
    match error.class() {
        // An unusable `PYTHONHASHSEED` is a process start-up fatal, not a Python
        // exception class: carry the original Kernel cause as a typed Native
        // start-up/set-policy failure.
        python_yaml::ErrorClass::Process => ActivationReadError::Native(
            crate::offering::NativeOfferingFailure::ActivationSetOrder {
                phase: "activation document".to_owned(),
                path: path.to_path_buf(),
                cause: match error {
                    python_yaml::YamlError::HashSeed(cause) => *cause,
                    _ => sk_kernel::python_yaml::HashSeedError,
                },
            },
        ),
        python_yaml::ErrorClass::Yaml | python_yaml::ErrorClass::Transport => DoctrineError::Yaml {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
        .into(),
        _ => ActivationReadError::Aborted {
            phase: "activation document".to_owned(),
            path: path.to_path_buf(),
            python: error.python_class().to_owned(),
            message: error.to_string(),
        },
    }
}

/// Read one activation document through the canonical Kernel Python-safe reader
/// (`ruamel.yaml`'s YAML-1.2 dialect, the one every activation file uses), `None`
/// when the file is absent.
fn read_exact_document(
    path: &Path,
) -> Result<Option<python_yaml::GraphDocument>, ActivationReadError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(DoctrineError::io("read activation document", path, source).into());
        }
    };
    let text = python_yaml::read_document_text(&bytes, Dialect::RuamelSafe12)
        .map_err(|error| map_document_error(path, &error))?;
    let graph = python_yaml::decode_graph(&text, Dialect::RuamelSafe12)
        .map_err(|error| map_document_error(path, &error))?;
    let document = python_yaml::GraphDocument::new(graph);
    // Source _load_mapping treats an empty/null document as {}, preserving the
    // separately observed file-exists fact in the ledger model.
    if document.root().is_null() {
        return Ok(None);
    }
    if document.root().as_mapping().is_none() && document.root().as_ordered_dict().is_none() {
        return Err(DoctrineError::YamlRootNotMapping {
            path: path.to_path_buf(),
        }
        .into());
    }
    Ok(Some(document))
}

/// Load the typed activation state for `repo_root` **exactly**.
///
/// Same declaration, same builder and same three-state/L1/L2 rules as
/// [`load_activation_ledger`], read through the canonical Kernel Python-YAML
/// graph, so the Source `str` code points survive (lone surrogates, NUL, PUA and
/// noncharacters included). The Source warning stages are delivered by default.
///
/// # Errors
///
/// A typed doctrine refusal, the real cause of a failed warning delivery, or the
/// [`ActivationReadError::Aborted`] runtime exception the pinned Source loader
/// would raise.
pub fn load_activation_ledger_exact(
    repo_root: &Path,
) -> Result<ExactActivationLedger, ActivationReadError> {
    load_activation_ledger_exact_with(repo_root, LedgerReadOptions::REPORT)
}

/// The exact Source inspection read: both warning stages are suppressed.
pub fn load_activation_ledger_exact_inspection(
    repo_root: &Path,
) -> Result<ExactActivationLedger, ActivationReadError> {
    load_activation_ledger_exact_with(repo_root, LedgerReadOptions::INSPECT)
}

/// Load the exact ledger with Source's `report_stray` option.
///
/// # Errors
///
/// A typed doctrine refusal, the real cause of a failed warning delivery, or the
/// runtime exception the pinned Source loader would raise.
pub fn load_activation_ledger_exact_with(
    repo_root: &Path,
    options: LedgerReadOptions,
) -> Result<ExactActivationLedger, ActivationReadError> {
    let resolved = resolve_activation_source(repo_root)?;
    let codec = OsEncodingContext::from_environment();
    match resolved.source() {
        ActivationSource::Legacy => {
            let path = legacy_probe_path(repo_root, &resolved)?;
            let document = read_exact_document(&path)?;
            // Source's stage: raw legacy read, then the deny report, then the
            // allow projection and validation.
            if options.report_stray {
                let keys = match &document {
                    Some(document) => graph_text_keys(&path, document)?,
                    None => Vec::new(),
                };
                report_legacy_deny_keys(repo_root, &path, keys, &codec)?;
            }
            let allow_keys = legacy_allow_keys();
            let view = match &document {
                Some(document) => FilteredView {
                    inner: &GraphView {
                        document,
                        path: path.clone(),
                    },
                    allowed: &allow_keys,
                },
                None => FilteredView {
                    inner: &EmptyView,
                    allowed: &allow_keys,
                },
            };
            let mut ledger = build_ledger(
                path.clone(),
                ActivationSource::Legacy,
                path.is_file(),
                &view,
                false,
            )?;
            ledger.ignored_legacy_deny_keys = match &document {
                Some(document) => {
                    let mut keys = Vec::new();
                    for key in (GraphView {
                        document,
                        path: path.clone(),
                    })
                    .keys()?
                    {
                        if let LedgerKey::Text(text) = key
                            && key_is_deny(&text)
                        {
                            keys.push(text);
                        }
                    }
                    keys.sort();
                    keys
                }
                None => Vec::new(),
            };
            Ok(ledger)
        }
        ActivationSource::Ledger => {
            // Source's stage: the stray report precedes the ledger read.
            if options.report_stray {
                report_stray_legacy_keys(repo_root, &resolved.stray_legacy_keys, &codec)?;
            }
            let path = activation_ledger_path(repo_root);
            let exists = sk_kernel::paths::source_path_is_file(&path)
                .map_err(|error| DoctrineError::io("stat activation ledger", &path, error))?;
            let document = if exists {
                read_exact_document(&path)?
            } else {
                None
            };
            let mut ledger = match &document {
                Some(document) => {
                    let view = GraphView {
                        document,
                        path: path.clone(),
                    };
                    build_ledger(
                        path,
                        ActivationSource::Ledger,
                        resolved.ledger_exists,
                        &view,
                        true,
                    )?
                }
                None => build_ledger(
                    path,
                    ActivationSource::Ledger,
                    resolved.ledger_exists,
                    &EmptyView,
                    true,
                )?,
            };
            ledger.stray_legacy_keys = resolved.stray_legacy_keys;
            Ok(ledger)
        }
    }
}

/// One identity spelling set for a denied id, over one token instantiation.
///
/// [`DenySet`] is the scalar capability view; [`ExactDenySet`] is the Source data.
/// Membership is tested against stems, declared ids and URNs, so a stem deny
/// subtracts a declared-id allow entry (R4).
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct DenySet<T = String> {
    /// File stems.
    pub stems: BTreeSet<T>,
    /// Declared artifact ids.
    pub declared_ids: BTreeSet<T>,
    /// Full URNs.
    pub urns: BTreeSet<T>,
}

/// The exact deny set: Source `str` code points, lone surrogates included.
pub type ExactDenySet = DenySet<PythonString>;

impl<T> Default for DenySet<T> {
    fn default() -> Self {
        Self {
            stems: BTreeSet::new(),
            declared_ids: BTreeSet::new(),
            urns: BTreeSet::new(),
        }
    }
}

impl<T: LedgerToken> DenySet<T> {
    /// The empty deny set.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Whether `token` matches any spelling of a denied id.
    pub fn admits(&self, token: &str) -> bool {
        !self.stems.iter().any(|item| item.matches_text(token))
            && !self
                .declared_ids
                .iter()
                .any(|item| item.matches_text(token))
            && !self.urns.iter().any(|item| item.matches_text(token))
    }

    /// Whether an exact code-point `token` matches any spelling of a denied id.
    ///
    /// On the exact set this is real code-point membership; on the scalar view a
    /// token with no scalar projection cannot equal a stored scalar spelling (it
    /// is never substituted with an empty token).
    pub fn admits_exact(&self, token: &PythonString) -> bool {
        !self.stems.iter().any(|item| item.matches_exact(token))
            && !self
                .declared_ids
                .iter()
                .any(|item| item.matches_exact(token))
            && !self.urns.iter().any(|item| item.matches_exact(token))
    }

    /// Every spelling held.
    pub fn spellings(&self) -> BTreeSet<T> {
        self.stems
            .union(&self.declared_ids)
            .chain(self.urns.iter())
            .cloned()
            .collect()
    }

    /// Whether the set holds anything.
    pub fn is_empty(&self) -> bool {
        self.stems.is_empty() && self.declared_ids.is_empty() && self.urns.is_empty()
    }

    fn union(&mut self, other: Self) {
        self.stems.extend(other.stems);
        self.declared_ids.extend(other.declared_ids);
        self.urns.extend(other.urns);
    }
}

/// A deny token's code-point spellings, or the deny algebra's own precondition.
///
/// Source's `IdentityIndex` lookup raises while it scans the queried kind (an
/// invalid `!!timestamp` in one artifact file, for example); that abort is the
/// reference's phase and must escape instead of becoming an unresolved healthy
/// spelling.
pub trait ExactIdentityLookup {
    /// Declared id, exactly, or the kind's scan abort.
    fn try_declared_id_exact(
        &self,
        kind: ArtifactKind,
        token: &PythonString,
    ) -> Result<Option<PythonString>, IdentityFailure>;
    /// Activation stem, exactly, or the kind's scan abort.
    fn try_stem_exact(
        &self,
        kind: ArtifactKind,
        token: &PythonString,
    ) -> Result<Option<PythonString>, IdentityFailure>;
}

impl ExactIdentityLookup for IdentityIndex {
    fn try_declared_id_exact(
        &self,
        kind: ArtifactKind,
        token: &PythonString,
    ) -> Result<Option<PythonString>, IdentityFailure> {
        IdentityIndex::try_declared_id_exact(self, kind, token)
    }

    fn try_stem_exact(
        &self,
        kind: ArtifactKind,
        token: &PythonString,
    ) -> Result<Option<PythonString>, IdentityFailure> {
        IdentityIndex::try_stem_exact(self, kind, token)
    }
}

/// The total (inspection) identity lookups, as an exact lookup that cannot abort.
struct ScalarIdentity<'a>(&'a dyn IdentityLookup);

impl ExactIdentityLookup for ScalarIdentity<'_> {
    fn try_declared_id_exact(
        &self,
        kind: ArtifactKind,
        token: &PythonString,
    ) -> Result<Option<PythonString>, IdentityFailure> {
        Ok(token.to_scalar_string().ok().and_then(|token| {
            self.0
                .declared_id(kind, &token)
                .map(|declared| PythonString::from_scalars(&declared))
        }))
    }

    fn try_stem_exact(
        &self,
        kind: ArtifactKind,
        token: &PythonString,
    ) -> Result<Option<PythonString>, IdentityFailure> {
        Ok(token.to_scalar_string().ok().and_then(|token| {
            self.0
                .stem(kind, &token)
                .map(|stem| PythonString::from_scalars(&stem))
        }))
    }
}

/// `<kind>:<id>` with *id* verbatim (Source `f"{kind.value}:{declared}"`).
fn urn_raw(kind: ArtifactKind, id: &PythonString) -> PythonString {
    let mut codepoints: Vec<u32> = kind.canonical().chars().map(u32::from).collect();
    codepoints.push(u32::from(':'));
    codepoints.extend_from_slice(id.codepoints());
    PythonString::from_codepoints(&codepoints).expect("kind prefix and id are valid code points")
}

/// `<kind>:<id>` with the Source `artifact_to_urn` normalisation (directives only).
fn urn_for(kind: ArtifactKind, id: &PythonString) -> PythonString {
    if kind == ArtifactKind::Directive {
        urn_raw(kind, &crate::org::normalize_directive_id_exact(id))
    } else {
        urn_raw(kind, id)
    }
}

/// The ONE spelling rule: a known token's stem / declared id / URN, or an unknown
/// token's own stem plus the minted URN.
fn spell_exact(
    kind: ArtifactKind,
    token: &PythonString,
    identity: &dyn ExactIdentityLookup,
) -> Result<ExactDenySet, IdentityFailure> {
    match identity.try_declared_id_exact(kind, token)? {
        None => Ok(ExactDenySet {
            stems: BTreeSet::from([token.clone()]),
            declared_ids: BTreeSet::new(),
            urns: BTreeSet::from([urn_for(kind, token)]),
        }),
        Some(declared) => {
            let stem = identity
                .try_stem_exact(kind, token)?
                .unwrap_or_else(|| token.clone());
            Ok(ExactDenySet {
                stems: BTreeSet::from([stem]),
                declared_ids: BTreeSet::from([declared.clone()]),
                urns: BTreeSet::from([urn_raw(kind, &declared)]),
            })
        }
    }
}

/// The scalar projection of an exact set (the explicit checked capability view).
fn scalar_view(set: ExactDenySet) -> DenySet<String> {
    let project = |items: BTreeSet<PythonString>| -> BTreeSet<String> {
        items
            .into_iter()
            .filter_map(|item| item.to_scalar_string().ok())
            .collect()
    };
    DenySet {
        stems: project(set.stems),
        declared_ids: project(set.declared_ids),
        urns: project(set.urns),
    }
}

/// The [`ExactDenySet`] of `tokens`, expanded through the exact identity index.
///
/// # Errors
///
/// The identity scan abort of a queried kind.
pub fn resolve_deny_exact(
    kind: ArtifactKind,
    tokens: impl IntoIterator<Item = PythonString>,
    identity: &dyn ExactIdentityLookup,
) -> Result<ExactDenySet, IdentityFailure> {
    let mut set = ExactDenySet::empty();
    for token in tokens {
        set.union(spell_exact(kind, &token, identity)?);
    }
    Ok(set)
}

/// `D* = resolve_deny(D) − R` in identity space, exactly.
///
/// # Errors
///
/// The identity scan abort of a queried kind.
pub fn effective_deny_exact(
    kind: ArtifactKind,
    deny: impl IntoIterator<Item = PythonString>,
    required_floor: impl IntoIterator<Item = PythonString>,
    identity: &dyn ExactIdentityLookup,
) -> Result<ExactDenySet, IdentityFailure> {
    let floor = resolve_deny_exact(kind, required_floor, identity)?.spellings();
    let mut set = ExactDenySet::empty();
    for token in deny {
        let spelled = spell_exact(kind, &token, identity)?;
        if spelled.spellings().is_disjoint(&floor) {
            set.union(spelled);
        }
    }
    Ok(set)
}

/// The [`DenySet`] of `tokens`, expanded through an inspection identity index.
pub fn resolve_deny(
    kind: ArtifactKind,
    tokens: impl IntoIterator<Item = String>,
    identity: &dyn IdentityLookup,
) -> DenySet {
    let exact_identity = ScalarIdentity(identity);
    let mut set = DenySet::empty();
    for token in tokens {
        if let Ok(spelled) = spell_exact(kind, &PythonString::from_scalars(&token), &exact_identity)
        {
            set.union(scalar_view(spelled));
        }
    }
    set
}

/// `D* = resolve_deny(D) − R` in identity space (the inspection view).
pub fn effective_deny(
    kind: ArtifactKind,
    deny: impl IntoIterator<Item = String>,
    required_floor: impl IntoIterator<Item = String>,
    identity: &dyn IdentityLookup,
) -> DenySet {
    let floor = resolve_deny(kind, required_floor, identity).spellings();
    let mut set = DenySet::empty();
    for token in deny {
        let spelled = resolve_deny(kind, [token], identity);
        if spelled.spellings().is_disjoint(&floor) {
            set.union(spelled);
        }
    }
    set
}

pub use crate::org_charter::RequiringPack;

/// The one refusal seam for an org-required artifact.
pub fn check_deactivation_permitted(
    kind: &str,
    artifact_id: &str,
    required_by: &BTreeMap<String, Vec<RequiringPack>>,
) -> Result<(), DoctrineError> {
    let Some(requiring) = required_by.get(artifact_id) else {
        return Ok(());
    };
    if requiring.is_empty() {
        return Ok(());
    }
    let kind_token = sk_charter::ArtifactKind::from_operator_token(kind)?;
    let policy_key = format!("required_{}", kind_token.plural());
    let packs = requiring
        .iter()
        .map(RequiringPack::describe)
        .collect::<Vec<_>>()
        .join(", ");
    let noun = if requiring.len() == 1 {
        "org pack"
    } else {
        "org packs"
    };
    Err(DoctrineError::RequiredArtifact {
        kind: kind.to_owned(),
        artifact_id: artifact_id.to_owned(),
        policy_key,
        requiring: format!("{noun} {packs}"),
    })
}
/// The explicit choices that may change a deny-list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DenyListChoice {
    /// An operator activation clearing an exclusion.
    Activate,
    /// An operator deactivation recording an exclusion.
    Deactivate,
    /// `--include-deactivated` during a cascade.
    CascadeIncludeDeactivated,
    /// `--preset --force`.
    PresetForce,
    /// The project-intent rename (narrowing only).
    Rename,
}

/// Refuse a deny write on an unmigrated project before any write or read.
pub fn refuse_unmigrated_deny_write(repo_root: &Path) -> Result<(), DoctrineError> {
    let resolved = resolve_activation_source(repo_root)?;
    if !resolved.unmigrated() {
        return Ok(());
    }
    let mut keys = BTreeSet::new();
    for (_, file_keys) in &resolved.unmigrated_keys {
        keys.extend(file_keys.iter().cloned());
    }
    let files = resolved
        .unmigrated_keys
        .iter()
        .map(|(path, _)| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(DoctrineError::UnmigratedDenyWrite {
        keys: keys.into_iter().collect::<Vec<_>>().join(", "),
        files,
    })
}

fn deny_set(value: Option<&Vec<String>>) -> BTreeSet<String> {
    value
        .map(|ids| ids.iter().cloned().collect())
        .unwrap_or_default()
}

type DenyChanges = Vec<(String, Vec<String>, Vec<String>)>;

fn deny_changes(before: &YamlDocument, after: &YamlDocument) -> Result<DenyChanges, DoctrineError> {
    let mut changes = Vec::new();
    for kind in activatable_kinds() {
        let key = deny_key(kind)?;
        let old = deny_set(before.get_string_list(&key)?.as_ref());
        let new = deny_set(after.get_string_list(&key)?.as_ref());
        if old != new {
            changes.push((
                key,
                new.difference(&old).cloned().collect(),
                old.difference(&new).cloned().collect(),
            ));
        }
    }
    Ok(changes)
}

/// Prepare the one activation-ledger write.
///
/// Values map a known ledger key to its post-state (`None` removes it);
/// `remove` additionally deletes keys. An empty deny list removes the key.
/// The deny-change guard runs before any bytes are prepared: a changed
/// deny-list without an explicit choice is refused and nothing is written.
pub fn prepare_ledger_write(
    repo_root: &Path,
    values: &BTreeMap<String, Option<Vec<String>>>,
    remove: &[String],
    deny_change: Option<DenyListChoice>,
) -> Result<PreparedWrite, DoctrineError> {
    let known = ledger_key_set();
    for key in values.keys().chain(remove.iter()) {
        if !known.contains(key) {
            return Err(DoctrineError::UnknownLedgerKey {
                path: repo_root.join(ACTIVATION_LEDGER_REL),
                key: key.clone(),
                allowed: ledger_keys().join(", "),
            });
        }
    }
    let resolved = resolve_activation_source(repo_root)?;
    if (values.keys().chain(remove.iter())).any(|key| key.starts_with(DENY_PREFIX))
        && resolved.unmigrated()
    {
        refuse_unmigrated_deny_write(repo_root)?;
    }
    let target = activation_ledger_path(repo_root);
    let document = YamlDocument::open(&target)?;
    let existed = target.is_file();
    // L1/L2 validation of the existing document happens on the same read.
    let typed: ActivationLedger = build_ledger(
        target.clone(),
        ActivationSource::Ledger,
        existed,
        &YamlView(&document),
        true,
    )?;
    let _ = typed;
    let before = YamlDocument::parse(document.original(), &target)?;
    let mut after = document;
    for key in ledger_keys() {
        if let Some(value) = values.get(&key) {
            match value {
                Some(ids) if key.starts_with(DENY_PREFIX) => {
                    let mut unique: Vec<String> = ids
                        .iter()
                        .cloned()
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect();
                    unique.sort();
                    if unique.is_empty() {
                        after.remove(&key);
                    } else {
                        after.set_string_list(&key, &unique);
                    }
                }
                Some(ids) => after.set_string_list(&key, ids),
                None => after.remove(&key),
            }
        } else if remove.contains(&key) {
            after.remove(&key);
        }
    }
    let changes = deny_changes(&before, &after)?;
    if !changes.is_empty() {
        if deny_change.is_none() {
            let (key, added, removed) = changes[0].clone();
            return Err(DoctrineError::DenyChangeRefused {
                key,
                added,
                removed,
            });
        }
        if deny_change == Some(DenyListChoice::Rename) {
            for (key, added, removed) in &changes {
                if !is_stem_rename(added, removed) {
                    return Err(DoctrineError::DenyChangeRefused {
                        key: key.clone(),
                        added: added.clone(),
                        removed: removed.clone(),
                    });
                }
            }
        }
    }
    let desired = if after.keys().is_empty() {
        if existed {
            b"{}\n".to_vec()
        } else {
            Vec::new()
        }
    } else {
        after.to_ledger_text()?.into_bytes()
    };
    PreparedWrite::prepare(&target, desired).map(|write| write.with_synthesis_lock(repo_root))
}

fn is_stem_rename(added: &[String], removed: &[String]) -> bool {
    const LEGACY_SLUG: &str = "mission-type-scope-directive";
    const NEW_SLUG: &str = "project-intent-directive";
    let mut renamed = BTreeSet::new();
    for token in removed {
        let Some((number, slug)) = token.split_once('-') else {
            return false;
        };
        if slug != LEGACY_SLUG || number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        renamed.insert(format!("{number}-{NEW_SLUG}"));
    }
    added.iter().all(|token| renamed.contains(token))
}

/// Apply the prepared ledger write under the standard lock.
pub fn apply_ledger_write(
    prepared: PreparedWrite,
) -> Result<crate::yaml::WriteOutcome, DoctrineError> {
    prepared.apply(crate::yaml::DEFAULT_LOCK_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::IdentityIndex;
    use std::fs;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    fn fixture_root() -> (tempfile::TempDir, IdentityIndex) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let pack = root.join("packs/built-in");
        write(
            &pack.join("directives/001-arch.directive.yaml"),
            "id: ARCH_001\n",
        );
        write(
            &pack.join("directives/002-style.directive.yaml"),
            "id: STYLE_002\n",
        );
        write(&pack.join("pack.yaml"), "name: built-in\n");
        let identity = IdentityIndex::from_roots(&pack, &[], None);
        (directory, identity)
    }

    #[test]
    fn keys_are_derived_from_the_artifact_taxonomy() {
        assert_eq!(
            allow_key(ArtifactKind::Directive).unwrap(),
            "activated_directives"
        );
        assert_eq!(
            deny_key(ArtifactKind::AgentProfile).unwrap(),
            "deactivated_agent_profiles"
        );
        assert!(allow_key(ArtifactKind::Template).is_err());
        let keys = ledger_keys();
        assert_eq!(keys[0], "activated_kinds");
        assert_eq!(keys[1], "mission_type_activations");
        assert!(keys.contains(&"deactivated_skills".to_owned()));
    }

    #[test]
    fn fresh_project_has_absent_three_state_allow() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = load_activation_ledger(directory.path()).unwrap();
        assert_eq!(ledger.source, ActivationSource::Ledger);
        assert!(!ledger.exists);
        assert_eq!(ledger.allow_for(ArtifactKind::Directive), None);
        assert!(ledger.deny_for(ArtifactKind::Directive).is_empty());
    }

    #[test]
    fn unknown_non_deny_key_fails_closed_but_unknown_deny_keys_survive() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            &root.join(ACTIVATION_LEDGER_REL),
            "typo_key: []\ndeactivated_mystery: [x]\n",
        );
        let error = load_activation_ledger(root).unwrap_err();
        assert!(matches!(error, DoctrineError::UnknownLedgerKey { .. }));

        write(
            &root.join(ACTIVATION_LEDGER_REL),
            "deactivated_mystery: [x]\n",
        );
        let ledger = load_activation_ledger(root).unwrap();
        assert_eq!(ledger.unknown_deny_keys, vec!["deactivated_mystery"]);
    }

    #[test]
    fn unknown_deny_keys_round_trip_through_raw() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            &root.join(ACTIVATION_LEDGER_REL),
            "deactivated_mystery: [x, y]\nactivated_directives: [001-arch]\n",
        );
        let ledger = load_activation_ledger(root).unwrap();
        let raw = ledger.raw();
        assert_eq!(
            raw.get("deactivated_mystery"),
            Some(&Some(vec!["x".to_owned(), "y".to_owned()]))
        );
        // A write through the seam keeps the unknown key's value intact.
        let values = BTreeMap::from([("activated_tactics".to_owned(), Some(vec!["t".to_owned()]))]);
        prepare_ledger_write(root, &values, &[], None)
            .unwrap()
            .apply(crate::yaml::DEFAULT_LOCK_TIMEOUT)
            .unwrap();
        let text = fs::read_to_string(root.join(ACTIVATION_LEDGER_REL)).unwrap();
        assert!(text.contains("deactivated_mystery"));
        let document = YamlDocument::parse(&text, &root.join(ACTIVATION_LEDGER_REL)).unwrap();
        assert_eq!(
            document.get_string_list("deactivated_mystery").unwrap(),
            Some(vec!["x".to_owned(), "y".to_owned()])
        );
    }

    #[test]
    fn malformed_values_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            &root.join(ACTIVATION_LEDGER_REL),
            "activated_directives: 7\n",
        );
        assert!(matches!(
            load_activation_ledger(root),
            Err(DoctrineError::InvalidLedgerValue { .. })
        ));
    }

    #[test]
    fn legacy_surface_projects_allow_keys_and_never_applies_deny() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(&root.join(CONFIG_YAML_REL), "charter: charter.yaml\n");
        write(
            &root.join("charter.yaml"),
            "activated_directives: [001-arch]\nactivated_tactics: [t]\ndeactivated_directives: [002-style]\n",
        );
        let resolved = resolve_activation_source(root).unwrap();
        assert_eq!(resolved.source(), ActivationSource::Legacy);
        assert!(resolved.unmigrated());
        let ledger = load_activation_ledger(root).unwrap();
        assert_eq!(
            ledger.allow_for(ArtifactKind::Directive),
            Some(&["001-arch".to_owned()][..])
        );
        assert_eq!(
            ledger.allow_for(ArtifactKind::Tactic),
            Some(&["t".to_owned()][..])
        );
        assert!(ledger.deny_for(ArtifactKind::Directive).is_empty());
        assert_eq!(
            ledger.ignored_legacy_deny_keys,
            vec!["deactivated_directives"]
        );
    }

    #[test]
    fn dangling_charter_pointer_and_non_mapping_config_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(&root.join(CONFIG_YAML_REL), "charter: missing.yaml\n");
        let error = load_activation_ledger(root).unwrap_err();
        assert!(error.to_string().contains("missing.yaml"));

        write(&root.join(CONFIG_YAML_REL), "- not\n- a mapping\n");
        let error = load_activation_ledger(root).unwrap_err();
        assert!(matches!(error, DoctrineError::YamlRootNotMapping { .. }));
    }

    #[test]
    fn deny_sets_resolve_through_identity_spellings() {
        let (directory, identity) = fixture_root();
        let deny = resolve_deny(
            ArtifactKind::Directive,
            vec!["001-arch".to_owned(), "STYLE_002".to_owned()],
            &identity,
        );
        assert!(!deny.admits("001-arch"));
        assert!(!deny.admits("ARCH_001"));
        assert!(!deny.admits("directive:ARCH_001"));
        assert!(!deny.admits("STYLE_002"));
        assert!(!deny.admits("directive:STYLE_002"));

        let floor = effective_deny(
            ArtifactKind::Directive,
            vec!["001-arch".to_owned()],
            vec!["ARCH_001".to_owned()],
            &identity,
        );
        assert!(floor.is_empty());
        drop(directory);
    }

    #[test]
    fn deny_write_without_choice_is_refused_and_writes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let ledger_path = root.join(ACTIVATION_LEDGER_REL);
        write(&ledger_path, "# keep\nactivated_directives: []\n");
        let before = fs::read_to_string(&ledger_path).unwrap();
        let values = BTreeMap::from([(
            "deactivated_directives".to_owned(),
            Some(vec!["001-arch".to_owned()]),
        )]);
        let error = prepare_ledger_write(root, &values, &[], None).unwrap_err();
        assert!(matches!(error, DoctrineError::DenyChangeRefused { .. }));
        assert_eq!(fs::read_to_string(&ledger_path).unwrap(), before);

        let prepared =
            prepare_ledger_write(root, &values, &[], Some(DenyListChoice::Deactivate)).unwrap();
        assert!(prepared.changed());
        prepared.apply(crate::yaml::DEFAULT_LOCK_TIMEOUT).unwrap();
        let after = fs::read_to_string(&ledger_path).unwrap();
        assert!(after.contains("# keep"));
        assert!(after.contains("001-arch"));
    }

    #[test]
    fn deny_write_on_unmigrated_project_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            &root.join(CONFIG_YAML_REL),
            "activated_directives: [001-arch]\n",
        );
        let error = refuse_unmigrated_deny_write(root).unwrap_err();
        assert!(matches!(error, DoctrineError::UnmigratedDenyWrite { .. }));
        let values = BTreeMap::from([(
            "deactivated_directives".to_owned(),
            Some(vec!["001-arch".to_owned()]),
        )]);
        assert!(
            prepare_ledger_write(root, &values, &[], Some(DenyListChoice::Deactivate)).is_err()
        );
        assert!(!root.join(ACTIVATION_LEDGER_REL).exists());
    }

    #[test]
    fn required_artifact_deactivation_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let _ = directory;
        let requiring = BTreeMap::from([(
            "001-arch".to_owned(),
            vec![RequiringPack {
                pack: "acme".to_owned(),
                via: None,
            }],
        )]);
        let error = check_deactivation_permitted("directive", "001-arch", &requiring).unwrap_err();
        assert!(matches!(error, DoctrineError::RequiredArtifact { .. }));
        assert!(check_deactivation_permitted("directive", "002-style", &requiring).is_ok());
    }
}
