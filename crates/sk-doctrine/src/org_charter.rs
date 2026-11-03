//! Org-charter policy import: `required_<kind>` floors, provenance and the
//! org-tier mission overrides.
//!
//! Mirrors `src/charter/activation/org_charter.py`,
//! `org_pack_discovery.py`, `org_expected_artifacts.py` and
//! `org_requirement_kinds.py`.
//!
//! The floor is the union of every declared org pack's `required_<kind>` list
//! over the pack's `extends:` chain, first-seen order, normalized to activation
//! stems. It is the `R` in `D* = D − R` (see
//! [`crate::ledger::effective_deny`]), so a stale project deny can never remove
//! an org-required artifact from a cascade.
//!
//! A missing or unreadable *declared* pack is a refusal in strict mode; the
//! lenient mode is the best-effort diagnostics path used while rendering.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;
use sk_charter::ArtifactKind;

use crate::error::DoctrineError;
use crate::identity::IdentityIndex;

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
pub(crate) fn built_in_search(anchor: &Path) -> sk_charter::BuiltInSearch {
    let mut search = sk_charter::BuiltInSearch::from_env(anchor);
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
    pub field: String,
    /// The policy value.
    pub value: String,
    /// The enforcement level (`advisory` today).
    pub enforcement: String,
}

/// One `activations` registry entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationEntry {
    /// The activation context.
    pub context: String,
    /// The declaring charter pack id.
    pub charter_pack_id: String,
    /// The artifact id.
    pub artifact_id: String,
    /// The artifact kind.
    pub artifact_kind: String,
}

/// One org pack's parsed `org-charter.yaml`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrgCharterPolicy {
    /// The file it was read from.
    pub path: PathBuf,
    /// The pack root.
    pub pack_root: PathBuf,
    /// Declared schema version (1 or 2).
    pub schema_version: i64,
    /// The `extends:` base pack name.
    pub extends: Option<String>,
    /// The org name.
    pub org_name: Option<String>,
    /// Interview defaults.
    pub interview_defaults: BTreeMap<String, String>,
    /// `required_<plural>` lists, verbatim order.
    pub required: BTreeMap<ArtifactKind, Vec<String>>,
    /// The skill namespace prefix.
    pub skill_namespace: Option<String>,
    /// Governance policies.
    pub governance_policies: Vec<GovernancePolicy>,
    /// Activation registry entries.
    pub activations: Vec<ActivationEntry>,
}

impl OrgCharterPolicy {
    /// The `required_<plural>` list of `kind`.
    pub fn required_for(&self, kind: ArtifactKind) -> &[String] {
        self.required.get(&kind).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// The unioned required floor across every configured org pack.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RequiredFloor {
    /// Normalized activation stems per kind (first-seen order). The raw
    /// authored spelling lives in [`read_org_required_tokens`].
    pub tokens: BTreeMap<ArtifactKind, Vec<String>>,
    /// Normalized activation stem -> the packs that require it.
    pub provenance: BTreeMap<(ArtifactKind, String), Vec<RequiringPack>>,
    /// Ids that resolve in no readable layer (kept as written).
    pub unresolved: Vec<String>,
}

impl RequiredFloor {
    /// The floor for one kind in the shape [`crate::plan::ActivationInputs`] and
    /// [`crate::activation::ActivationState`] consume.
    pub fn into_map(self) -> BTreeMap<ArtifactKind, Vec<String>> {
        self.tokens
    }
}

fn yaml(path: &Path) -> Result<Value, DoctrineError> {
    let text = std::fs::read_to_string(path)
        .map_err(|source| DoctrineError::io("read org charter", path, source))?;
    serde_yaml_ng::from_str(&text).map_err(|error| DoctrineError::InvalidOrgCharter {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

fn string_list(
    path: &Path,
    mapping: &serde_yaml_ng::Mapping,
    key: &str,
) -> Result<Vec<String>, DoctrineError> {
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
        let Some(text) = item.as_str() else {
            return Err(DoctrineError::InvalidOrgCharter {
                path: path.to_path_buf(),
                message: format!("`{key}` must contain string ids only"),
            });
        };
        union_required_tokens(&mut out, [text]);
    }
    Ok(out)
}

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

/// The `required_<kind>` list of one raw document, leniently.
///
/// Mirrors `org_pack_discovery._required_values`: an absent/null field is empty,
/// and a non-list (or list with a non-string member) field is dropped **for that
/// field only**. A typo in one required kind must never shrink the floor for
/// every other kind, and never discard the pack's `extends:` contribution.
fn lenient_required_values(mapping: &serde_yaml_ng::Mapping, key: &str) -> Vec<String> {
    let Some(items) = mapping.get(key).and_then(Value::as_sequence) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let Some(text) = item.as_str() else {
            return Vec::new();
        };
        union_required_tokens(&mut out, [text]);
    }
    out
}

/// A lenient best-effort policy from the raw document.
///
/// Used by the lenient readers when strict schema parsing refuses the file: the
/// pack still contributes its per-field `required_<kind>` lists, `extends:`,
/// `org_name` and `skill_namespace`, exactly like the reference's raw mapping
/// iterator. Governance policies/interview defaults/activations are not
/// reconstructed here because only strict readers consume them.
fn lenient_policy(
    pack_root: &Path,
    path: PathBuf,
    mapping: &serde_yaml_ng::Mapping,
) -> OrgCharterPolicy {
    let mut required = BTreeMap::new();
    for kind in org_requirable_kinds() {
        required.insert(kind, lenient_required_values(mapping, &required_key(kind)));
    }
    let text = |key: &str| {
        mapping
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let schema_version = match mapping.get("schema_version") {
        Some(Value::String(text)) => text
            .trim()
            .parse::<i64>()
            .unwrap_or(ORG_CHARTER_SCHEMA_VERSION),
        Some(Value::Number(number)) => number.as_i64().unwrap_or(ORG_CHARTER_SCHEMA_VERSION),
        _ => ORG_CHARTER_SCHEMA_VERSION,
    };
    OrgCharterPolicy {
        path,
        pack_root: pack_root.to_path_buf(),
        schema_version,
        extends: text("extends"),
        org_name: text("org_name"),
        interview_defaults: BTreeMap::new(),
        required,
        skill_namespace: text("skill_namespace"),
        governance_policies: Vec::new(),
        activations: Vec::new(),
    }
}

/// Read one pack's raw `org-charter.yaml` mapping without validating it.
fn raw_org_charter_document(pack_root: &Path) -> Option<(PathBuf, serde_yaml_ng::Mapping)> {
    let path = pack_root.join(ORG_CHARTER);
    if !path.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    let value: Value = serde_yaml_ng::from_str(&text).ok()?;
    let mapping = value.as_mapping()?.clone();
    Some((path, mapping))
}

/// Read one pack's `org-charter.yaml`; `None` only when the file is absent
/// (or empty).
///
/// A present file with an unknown top-level key, a non-list `required_*`, or a
/// schema version outside 1..=2 is [`DoctrineError::InvalidOrgCharter`]: an
/// author who wrote a policy expects it to take effect, never to be dropped.
pub fn load_org_charter_policy(
    pack_root: &Path,
) -> Result<Option<OrgCharterPolicy>, DoctrineError> {
    let path = pack_root.join(ORG_CHARTER);
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|source| DoctrineError::io("read org charter", &path, source))?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    let value: Value =
        serde_yaml_ng::from_str(&text).map_err(|error| DoctrineError::InvalidOrgCharter {
            path: path.clone(),
            message: error.to_string(),
        })?;
    let Some(mapping) = value.as_mapping() else {
        return Err(DoctrineError::InvalidOrgCharter {
            path,
            message: "the org-charter.yaml root must be a mapping".to_owned(),
        });
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

    let schema_version = match mapping.get("schema_version") {
        None | Some(Value::Null) => ORG_CHARTER_SCHEMA_VERSION,
        Some(Value::Number(number)) => {
            number
                .as_i64()
                .ok_or_else(|| DoctrineError::InvalidOrgCharter {
                    path: path.clone(),
                    message: "schema_version must be an integer".to_owned(),
                })?
        }
        Some(Value::String(text)) => {
            text.parse::<i64>()
                .map_err(|_| DoctrineError::InvalidOrgCharter {
                    path: path.clone(),
                    message: "schema_version must be an integer or numeric string".to_owned(),
                })?
        }
        Some(other) => {
            return Err(DoctrineError::InvalidOrgCharter {
                path,
                message: format!("schema_version must be an integer, got {}", describe(other)),
            });
        }
    };
    // The pinned reference model types `schema_version` as an int and folds ANY
    // value (it is not a compatibility gate); refusing a higher version would be
    // a new validity refusal relative to upstream. Retired-field rejection and
    // the activation-entry shape still apply, so the field remains meaningful.
    let _ = schema_version;

    let extends = mapping
        .get("extends")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned);
    let org_name = mapping
        .get("org_name")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned);
    let skill_namespace = mapping
        .get("skill_namespace")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned);
    let mut interview_defaults = BTreeMap::new();
    if let Some(defaults) = mapping.get("interview_defaults") {
        let Some(defaults) = defaults.as_mapping() else {
            return Err(DoctrineError::InvalidOrgCharter {
                path,
                message: "interview_defaults must be a mapping".to_owned(),
            });
        };
        for (key, value) in defaults {
            let Some(key) = key.as_str() else {
                return Err(DoctrineError::InvalidOrgCharter {
                    path: path.clone(),
                    message: "interview_defaults keys must be strings".to_owned(),
                });
            };
            let text = match value {
                Value::String(text) => text.clone(),
                Value::Bool(flag) => flag.to_string(),
                _ => {
                    return Err(DoctrineError::InvalidOrgCharter {
                        path: path.clone(),
                        message: format!("interview_defaults[{key:?}] must be a string or boolean"),
                    });
                }
            };
            interview_defaults.insert(key.to_owned(), text);
        }
    }

    let mut required = BTreeMap::new();
    for kind in org_requirable_kinds() {
        required.insert(kind, string_list(&path, mapping, &required_key(kind))?);
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
            let field = entry.get("field").and_then(Value::as_str).ok_or_else(|| {
                DoctrineError::InvalidOrgCharter {
                    path: path.clone(),
                    message: "governance_policies entry is missing `field`".to_owned(),
                }
            })?;
            let value = match entry.get("value") {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Bool(flag)) => flag.to_string(),
                _ => {
                    return Err(DoctrineError::InvalidOrgCharter {
                        path: path.clone(),
                        message: "governance_policies `value` must be a string or boolean"
                            .to_owned(),
                    });
                }
            };
            let enforcement = entry
                .get("enforcement")
                .and_then(Value::as_str)
                .unwrap_or("advisory")
                .to_owned();
            governance_policies.push(GovernancePolicy {
                field: field.to_owned(),
                value,
                enforcement,
            });
        }
    }

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
            let field = |name: &str| -> Result<String, DoctrineError> {
                entry
                    .get(name)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| DoctrineError::InvalidOrgCharter {
                        path: path.clone(),
                        message: format!("activations entry is missing `{name}`"),
                    })
            };
            activations.push(ActivationEntry {
                context: field("activation_context")?,
                charter_pack_id: field("charter_pack_id")?,
                artifact_id: field("artifact_id")?,
                artifact_kind: field("artifact_kind")?,
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

/// Union `items` into `into`: stripped, non-empty, first-seen order.
///
/// This is THE merge rule for every `required_<kind>` field.
pub fn union_required_tokens(
    into: &mut Vec<String>,
    items: impl IntoIterator<Item = impl AsRef<str>>,
) {
    for item in items {
        let token = item.as_ref().trim();
        if !token.is_empty() && !into.iter().any(|existing| existing == token) {
            into.push(token.to_owned());
        }
    }
}

/// The configured org pack roots, in declaration order.
pub fn configured_org_roots(repo_root: &Path) -> Result<Vec<(String, PathBuf)>, DoctrineError> {
    configured_org_roots_with_packs_root(repo_root, packs_root_override().as_deref())
}

/// Configured org pack roots resolved against an explicit packs-root override.
///
/// `packs_root` accepts either the documented parent (`<...>/packs`) or the
/// built-in leaf (`<...>/packs/built-in`); both resolve the built-in tier, so
/// the org registry (and the required floor read from it) never disappears
/// because of the spelling.
pub fn configured_org_roots_with_packs_root(
    repo_root: &Path,
    packs_root: Option<&Path>,
) -> Result<Vec<(String, PathBuf)>, DoctrineError> {
    let config = sk_charter::load_project_config(repo_root, &sk_charter::Limits::default())?;
    let expansion = sk_charter::PathExpansion::from_env();
    let search = match packs_root {
        Some(root) => sk_charter::BuiltInSearch::from_env(repo_root)
            .with_packs_root_env(normalize_packs_root(root)),
        None => built_in_search(repo_root),
    };
    let roots = sk_charter::PackRoots::resolve(repo_root, &config, &expansion, Some(&search))?;
    Ok(roots
        .organizations
        .iter()
        .map(|pack| (pack.name.clone(), pack.root.clone()))
        .collect())
}

fn strict_error(message: String) -> DoctrineError {
    DoctrineError::InvalidOrgCharter {
        path: PathBuf::from("org pack registry"),
        message: format!(
            "{message}\nThe org required floor cannot be read, so nothing was changed. \
             Fix the org pack, or run `spec-kitty doctor charter-packs`."
        ),
    }
}

fn declared_org_roots(
    repo_root: &Path,
    strict: bool,
    packs_root: Option<&Path>,
) -> Result<Vec<(String, PathBuf)>, DoctrineError> {
    match configured_org_roots_with_packs_root(repo_root, packs_root) {
        Ok(roots) => Ok(roots),
        Err(error) if strict => Err(strict_error(format!(
            "The org pack registry in .kittify/config.yaml cannot be read: {error}"
        ))),
        Err(_) => Ok(Vec::new()),
    }
}

/// Resolve `declared`'s `extends:` chain, declared first then bases nearest
/// first.
fn chain_members(
    declared: &str,
    docs: &BTreeMap<String, OrgCharterPolicy>,
    strict: bool,
) -> Result<Vec<String>, DoctrineError> {
    let mut chain = vec![declared.to_owned()];
    let mut seen: BTreeSet<String> = BTreeSet::from([declared.to_owned()]);
    let mut current = declared.to_owned();
    while let Some(policy) = docs.get(&current) {
        let Some(base) = policy.extends.clone() else {
            break;
        };
        if !docs.contains_key(&base) {
            if strict {
                return Err(DoctrineError::OrgCharterExtends {
                    path: policy.path.clone(),
                    message: format!(
                        "Base pack '{base}' not found. Chain: {}",
                        chain.join(" -> ")
                    ),
                });
            }
            break;
        }
        if !seen.insert(base.clone()) {
            let mut cycle = chain.clone();
            cycle.push(base);
            if strict {
                return Err(DoctrineError::OrgCharterExtends {
                    path: policy.path.clone(),
                    message: format!("Cycle detected in extends: chain: {}", cycle.join(" -> ")),
                });
            }
            break;
        }
        chain.push(base.clone());
        current = base;
    }
    Ok(chain)
}

/// Every `(declared pack, member pack, policy)` the required-floor walk visits.
pub fn org_charter_documents(
    repo_root: &Path,
    strict: bool,
) -> Result<Vec<(String, String, OrgCharterPolicy)>, DoctrineError> {
    org_charter_documents_with_packs_root(repo_root, strict, packs_root_override().as_deref())
}

/// [`org_charter_documents`] resolved against an explicit packs-root override
/// (either the documented parent or the built-in leaf spelling).
pub fn org_charter_documents_with_packs_root(
    repo_root: &Path,
    strict: bool,
    packs_root: Option<&Path>,
) -> Result<Vec<(String, String, OrgCharterPolicy)>, DoctrineError> {
    let declared = declared_org_roots(repo_root, strict, packs_root)?;
    let mut docs: BTreeMap<String, OrgCharterPolicy> = BTreeMap::new();
    for (name, root) in &declared {
        if strict && !root.is_dir() {
            return Err(strict_error(format!(
                "org pack {name:?} is declared but its path is not an existing directory ({}); \
                 run `spec-kitty charter fetch --pack {name}`.",
                root.display()
            )));
        }
        match load_org_charter_policy(root) {
            Ok(Some(policy)) => {
                docs.entry(name.clone()).or_insert(policy);
            }
            Ok(None) => {}
            Err(error) if strict => return Err(error),
            Err(_) => {
                // Lenient: the reference reads the raw mapping field by field, so
                // one malformed field must not erase the pack's other required
                // kinds or its `extends:` contribution.
                if let Some((path, mapping)) = raw_org_charter_document(root) {
                    docs.entry(name.clone())
                        .or_insert_with(|| lenient_policy(root, path, &mapping));
                }
            }
        }
    }
    let mut out = Vec::new();
    for (name, _root) in &declared {
        if !docs.contains_key(name) {
            continue;
        }
        for member in chain_members(name, &docs, strict)? {
            let policy = docs[&member].clone();
            out.push((name.clone(), member, policy));
        }
    }
    Ok(out)
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

/// The required floor of `kind` as the raw ids the packs list (any spelling).
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
    let mut out = Vec::new();
    for (_declared, _member, policy) in
        org_charter_documents_with_packs_root(repo_root, strict, packs_root)?
    {
        union_required_tokens(&mut out, policy.required_for(kind));
    }
    Ok(out)
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

/// [`read_org_required_provenance`] resolved against an explicit packs-root
/// override (parent or built-in leaf spelling).
pub fn read_org_required_provenance_with_packs_root(
    repo_root: &Path,
    kind: ArtifactKind,
    identity: &IdentityIndex,
    strict: bool,
    packs_root: Option<&Path>,
) -> Result<BTreeMap<String, Vec<RequiringPack>>, DoctrineError> {
    let mut provenance: BTreeMap<String, Vec<RequiringPack>> = BTreeMap::new();
    for (declared, member, policy) in
        org_charter_documents_with_packs_root(repo_root, strict, packs_root)?
    {
        let ids = policy.required_for(kind);
        if ids.is_empty() {
            continue;
        }
        let requirer = RequiringPack {
            pack: member.clone(),
            via: (member != declared).then(|| declared.clone()),
        };
        for token in ids {
            let stem = identity.stored_spelling(kind, token)?;
            let entries = provenance.entry(stem).or_default();
            if !entries.contains(&requirer) {
                entries.push(requirer.clone());
            }
        }
    }
    Ok(provenance)
}

/// The org-required ids of `kind` as activation stems (the `--only` and
/// promotion writers' spelling).
///
/// An id that resolves in no readable layer is kept raw and named in
/// `warnings`; a declared id whose file name another layer uses for a different
/// id raises [`DoctrineError::ActivationIdCollision`].
pub fn org_required_stems(
    repo_root: &Path,
    kind: ArtifactKind,
    identity: &IdentityIndex,
    warnings: &mut Vec<String>,
) -> Result<Vec<String>, DoctrineError> {
    if !org_requirable_kinds().contains(&kind) {
        return Ok(Vec::new());
    }
    let raw = read_org_required_tokens(repo_root, kind, false)?;
    let mut stems = Vec::new();
    for token in &raw {
        if !identity.knows(kind, token) {
            warnings.push(format!(
                "Org-required {} {token:?} was not found in any readable layer; kept as written.",
                kind.canonical()
            ));
        }
        union_required_tokens(&mut stems, [identity.stored_spelling(kind, token)?]);
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

/// [`required_floor`] resolved against an explicit packs-root override.
///
/// The org registry is read through the same normalized built-in search as
/// every other root seam, so a `SPEC_KITTY_PACKS_ROOT` that names the built-in
/// leaf (accepted by the CLI adapter) cannot silently empty the floor.
pub fn required_floor_with_packs_root(
    repo_root: &Path,
    identity: &IdentityIndex,
    packs_root: Option<&Path>,
) -> Result<RequiredFloor, DoctrineError> {
    let mut floor = RequiredFloor::default();
    for kind in org_requirable_kinds() {
        for token in read_org_required_tokens_with_packs_root(repo_root, kind, false, packs_root)? {
            if !identity.knows(kind, &token) && !floor.unresolved.contains(&token) {
                floor.unresolved.push(token.clone());
            }
            let stem = identity.stored_spelling(kind, &token)?;
            let entries = floor.tokens.entry(kind).or_default();
            if !entries.contains(&stem) {
                entries.push(stem);
            }
        }
    }
    for kind in org_requirable_kinds() {
        for (stem, packs) in read_org_required_provenance_with_packs_root(
            repo_root, kind, identity, false, packs_root,
        )? {
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
    let tokens = read_org_required_tokens(repo_root, ArtifactKind::Directive, false)?;
    let unknown: Vec<String> = tokens
        .into_iter()
        .filter(|token| !identity.knows(ArtifactKind::Directive, token))
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

/// The org-tier `requirement-kinds.yaml` of `mission_type`, whole-file, the last
/// matching org root wins.
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
        let value = yaml(&path)?;
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

/// The org `skill_namespace`, last non-empty value across packs wins.
pub fn read_org_skill_namespace(repo_root: &Path) -> Result<Option<String>, DoctrineError> {
    let mut namespace: Option<String> = None;
    for (_name, policy) in declared_policies(repo_root, false)? {
        if let Some(candidate) = policy.skill_namespace
            && !candidate.trim().is_empty()
        {
            namespace = Some(candidate.trim().to_owned());
        }
    }
    Ok(namespace)
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
            policy.required_for(ArtifactKind::Directive),
            ["DIRECTIVE_001".to_owned(), "001-arch".to_owned()]
        );
        assert_eq!(policy.required_for(ArtifactKind::Skill), ["s1".to_owned()]);
        assert_eq!(
            policy.interview_defaults.get("strict").map(String::as_str),
            Some("true")
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
                .schema_version,
            9
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
        // Strict reading still refuses the malformed scalar field.
        assert!(
            org_charter_documents(root, true)
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
