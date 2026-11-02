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
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;

use crate::error::DoctrineError;
use crate::identity::IdentityLookup;
use crate::yaml::{PreparedWrite, YamlDocument};

/// The activation ledger, relative to the repository root.
pub const ACTIVATION_LEDGER_REL: &str = ".kittify/charter/activation.yaml";
/// `.kittify/config.yaml`, relative to the repository root.
pub const CONFIG_YAML_REL: &str = ".kittify/config.yaml";

const ALLOW_PREFIX: &str = "activated_";
const DENY_PREFIX: &str = "deactivated_";
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

/// The typed activation state of one project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationLedger {
    /// The backing file.
    pub path: PathBuf,
    /// Where the state was read from.
    pub source: ActivationSource,
    /// Whether the backing file exists.
    pub exists: bool,
    /// `activated_kinds`, or `None` when absent (all kinds active).
    pub activated_kinds: Option<Vec<String>>,
    /// `mission_type_activations`, or `None` when absent (nothing active).
    pub mission_type_activations: Option<Vec<String>>,
    /// Per-kind allow state; `None` is the three-state "absent".
    pub allow: BTreeMap<ArtifactKind, Option<Vec<String>>>,
    /// Per-kind deny list (empty when nothing is denied).
    pub deny: BTreeMap<ArtifactKind, Vec<String>>,
    /// Unknown `deactivated_<kind>` keys, kept verbatim on write.
    pub unknown_deny_keys: Vec<String>,
    /// Legacy keys found beside the ledger (ignored; the ledger wins).
    pub stray_legacy_keys: Vec<(PathBuf, Vec<String>)>,
    /// Legacy `deactivated_<kind>` keys that were reported but not applied.
    pub ignored_legacy_deny_keys: Vec<String>,
}

impl Default for ActivationLedger {
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
            stray_legacy_keys: Vec::new(),
            ignored_legacy_deny_keys: Vec::new(),
        }
    }
}

impl ActivationLedger {
    /// The allow state of `kind`.
    pub fn allow_for(&self, kind: ArtifactKind) -> Option<&[String]> {
        self.allow.get(&kind).and_then(Option::as_deref)
    }

    /// The deny list of `kind`.
    pub fn deny_for(&self, kind: ArtifactKind) -> &[String] {
        self.deny.get(&kind).map(Vec::as_slice).unwrap_or(&[])
    }

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
        raw
    }

    /// The three-state activated-kind set: `None` means every kind.
    pub fn activated_kinds_set(&self) -> Option<BTreeSet<String>> {
        self.activated_kinds
            .as_ref()
            .map(|kinds| kinds.iter().cloned().collect())
    }

    /// The mission types in force.
    pub fn mission_types(&self) -> BTreeSet<String> {
        self.mission_type_activations
            .as_ref()
            .map(|items| items.iter().cloned().collect())
            .unwrap_or_default()
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

fn legacy_keys_in(document: &YamlDocument, allowed: &BTreeSet<String>) -> Vec<String> {
    let mut keys: Vec<String> = document
        .keys()
        .into_iter()
        .filter(|key| allowed.contains(key))
        .collect();
    keys.sort();
    keys
}

/// Resolve where this repository's activation lives.
pub fn resolve_activation_source(
    repo_root: &Path,
) -> Result<ResolvedActivationSource, DoctrineError> {
    let config = config_document(repo_root)?;
    let (pointer, pointer_exists) = pointer_path(repo_root, config.get_str("charter"));
    let ledger_path = activation_ledger_path(repo_root);
    let ledger_exists = ledger_path.is_file();
    let ledger_keys = ledger_key_set();
    let mut found: Vec<(PathBuf, Vec<String>)> = Vec::new();
    let config_keys = legacy_keys_in(&config, &ledger_keys);
    if !config_keys.is_empty() {
        found.push((config.path().to_path_buf(), config_keys));
    }
    if let (Some(pointer), true) = (&pointer, pointer_exists) {
        let pointed = read_mapping(pointer, pointer)?;
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

fn list_or_error(
    document: &YamlDocument,
    key: &str,
    strict: bool,
) -> Result<Option<Vec<String>>, DoctrineError> {
    match document.get_string_list(key) {
        Ok(value) => Ok(value),
        Err(error) => {
            if strict {
                Err(error)
            } else {
                // A legacy surface has one old error text; keep the same typed
                // refusal so a malformed legacy value never reads as absent.
                Err(error)
            }
        }
    }
}

fn build_ledger(
    path: PathBuf,
    source: ActivationSource,
    exists: bool,
    document: &YamlDocument,
    strict: bool,
) -> Result<ActivationLedger, DoctrineError> {
    let known = ledger_key_set();
    let mut unknown_deny_keys = Vec::new();
    if strict {
        for key in document.keys() {
            if known.contains(&key) {
                continue;
            }
            if key.starts_with(DENY_PREFIX) {
                unknown_deny_keys.push(key);
                continue;
            }
            return Err(DoctrineError::UnknownLedgerKey {
                path: path.clone(),
                key,
                allowed: ledger_keys().join(", "),
            });
        }
    }
    let activated_kinds = list_or_error(document, ACTIVATED_KINDS, strict)?;
    let mission_type_activations = list_or_error(document, MISSION_TYPE_ACTIVATIONS, strict)?;
    let mut allow = BTreeMap::new();
    let mut deny = BTreeMap::new();
    for kind in activatable_kinds() {
        let key = allow_key(kind)?;
        let value = list_or_error(document, &key, strict)?;
        allow.insert(kind, value);
        let deny_list = list_or_error(document, &deny_key(kind)?, strict)?.unwrap_or_default();
        deny.insert(kind, deny_list);
    }
    let ignored_legacy_deny_keys = if source == ActivationSource::Legacy {
        let mut keys: Vec<String> = document
            .keys()
            .into_iter()
            .filter(|key| key.starts_with(DENY_PREFIX))
            .collect();
        keys.sort();
        keys
    } else {
        Vec::new()
    };
    Ok(ActivationLedger {
        path,
        source,
        exists,
        activated_kinds,
        mission_type_activations,
        allow,
        deny,
        unknown_deny_keys,
        stray_legacy_keys: Vec::new(),
        ignored_legacy_deny_keys,
    })
}

fn legacy_path_and_document(
    repo_root: &Path,
    resolved: &ResolvedActivationSource,
) -> Result<(PathBuf, YamlDocument), DoctrineError> {
    let config_path = repo_root.join(CONFIG_YAML_REL);
    match &resolved.pointer {
        Some(pointer) if resolved.pointer_exists => {
            let document = read_mapping(pointer, pointer)?;
            Ok((pointer.clone(), document))
        }
        Some(pointer) => Err(DoctrineError::Yaml {
            path: config_path,
            message: format!(
                ".kittify/config.yaml 'charter:' pointer names {}, which does not exist",
                pointer.display()
            ),
        }),
        None => {
            let config = config_document(repo_root)?;
            Ok((config_path, config))
        }
    }
}

/// Load the typed activation state for `repo_root`.
pub fn load_activation_ledger(repo_root: &Path) -> Result<ActivationLedger, DoctrineError> {
    let resolved = resolve_activation_source(repo_root)?;
    match resolved.source() {
        ActivationSource::Legacy => {
            let (path, document) = legacy_path_and_document(repo_root, &resolved)?;
            let allow_keys = legacy_allow_keys();
            let mut projected = YamlDocument::empty(&path);
            for key in allow_keys {
                if let Some(value) = document.get(&key) {
                    let _ = value;
                    let list = document.get_string_list(&key)?;
                    if let Some(ids) = list {
                        projected.set_string_list(&key, &ids);
                    }
                }
            }
            let mut ledger = build_ledger(
                path,
                ActivationSource::Legacy,
                document.exists_on_disk(),
                &projected,
                false,
            )?;
            ledger.ignored_legacy_deny_keys = document
                .keys()
                .into_iter()
                .filter(|key| key.starts_with(DENY_PREFIX))
                .collect();
            ledger.ignored_legacy_deny_keys.sort();
            Ok(ledger)
        }
        ActivationSource::Ledger => {
            let path = activation_ledger_path(repo_root);
            let document = read_mapping(&path, &path)?;
            let existed = path.is_file();
            let mut ledger =
                build_ledger(path, ActivationSource::Ledger, existed, &document, true)?;
            ledger.stray_legacy_keys = resolved.stray_legacy_keys;
            Ok(ledger)
        }
    }
}

trait ExistsOnDisk {
    fn exists_on_disk(&self) -> bool;
}

impl ExistsOnDisk for YamlDocument {
    fn exists_on_disk(&self) -> bool {
        self.path().is_file()
    }
}

/// One identity spelling set for a denied id.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DenySet {
    /// File stems.
    pub stems: BTreeSet<String>,
    /// Declared artifact ids.
    pub declared_ids: BTreeSet<String>,
    /// Full URNs.
    pub urns: BTreeSet<String>,
}

impl DenySet {
    /// The empty deny set.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Whether `token` matches any spelling of a denied id.
    pub fn admits(&self, token: &str) -> bool {
        !self.stems.contains(token)
            && !self.declared_ids.contains(token)
            && !self.urns.contains(token)
    }

    /// Every spelling held.
    pub fn spellings(&self) -> BTreeSet<String> {
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

    fn union(&mut self, other: DenySet) {
        self.stems.extend(other.stems);
        self.declared_ids.extend(other.declared_ids);
        self.urns.extend(other.urns);
    }
}

fn spell(kind: ArtifactKind, token: &str, identity: &dyn IdentityLookup) -> DenySet {
    match identity.declared_id(kind, token) {
        None => DenySet {
            stems: BTreeSet::from([token.to_owned()]),
            declared_ids: BTreeSet::new(),
            urns: BTreeSet::from([format!("{}:{}", kind.canonical(), token)]),
        },
        Some(declared) => {
            let stem = identity
                .stem(kind, token)
                .unwrap_or_else(|| token.to_owned());
            DenySet {
                stems: BTreeSet::from([stem]),
                declared_ids: BTreeSet::from([declared.clone()]),
                urns: BTreeSet::from([format!("{}:{}", kind.canonical(), declared)]),
            }
        }
    }
}

/// The [`DenySet`] of `tokens`, expanded through the identity index.
pub fn resolve_deny(
    kind: ArtifactKind,
    tokens: impl IntoIterator<Item = String>,
    identity: &dyn IdentityLookup,
) -> DenySet {
    let mut set = DenySet::empty();
    for token in tokens {
        set.union(spell(kind, &token, identity));
    }
    set
}

/// `D* = resolve_deny(D) − R` in identity space.
///
/// A deny entry any of whose spellings names a required-floor id is stale and
/// inert; the rest are expanded.
pub fn effective_deny(
    kind: ArtifactKind,
    deny: impl IntoIterator<Item = String>,
    required_floor: impl IntoIterator<Item = String>,
    identity: &dyn IdentityLookup,
) -> DenySet {
    let floor = resolve_deny(kind, required_floor, identity).spellings();
    let mut set = DenySet::empty();
    for token in deny {
        let spelled = spell(kind, &token, identity);
        if spelled.spellings().is_disjoint(&floor) {
            set.union(spelled);
        }
    }
    set
}

/// A pack that requires an artifact, so it cannot be deactivated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequiringPack {
    /// Pack name.
    pub pack: String,
    /// The policy file or source that declared it.
    pub source: String,
    /// The `required_<plural>` key.
    pub policy_key: String,
}

impl RequiringPack {
    /// A stable description for diagnostics.
    pub fn describe(&self) -> String {
        format!("{} ({})", self.pack, self.source)
    }
}

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
    let noun = if requiring.len() == 1 {
        "org pack"
    } else {
        "org packs"
    };
    let packs = requiring
        .iter()
        .map(RequiringPack::describe)
        .collect::<Vec<_>>()
        .join(", ");
    let _ = noun;
    Ok(()).and_then(|()| {
        Err(DoctrineError::RequiredArtifact {
            kind: kind.to_owned(),
            artifact_id: artifact_id.to_owned(),
            policy_key: policy_key.clone(),
            requiring: packs,
        })
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
    let typed = build_ledger(
        target.clone(),
        ActivationSource::Ledger,
        existed,
        &document,
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
        after.to_text().into_bytes()
    };
    PreparedWrite::prepare(&target, desired)
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
                source: "org-charter.yaml".to_owned(),
                policy_key: "required_directives".to_owned(),
            }],
        )]);
        let error = check_deactivation_permitted("directive", "001-arch", &requiring).unwrap_err();
        assert!(matches!(error, DoctrineError::RequiredArtifact { .. }));
        assert!(check_deactivation_permitted("directive", "002-style", &requiring).is_ok());
    }
}
