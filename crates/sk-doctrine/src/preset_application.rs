//! Apply a pack's activation preset to the active charter (FR-001, #3732).
//!
//! `spec-kitty charter activate [--pack <pack>] --preset <preset>` is a thin CLI
//! adapter over this module (C-007): [`plan_preset_application`] is a pure read
//! that computes the whole change, and [`apply_preset_plan`] performs it as
//! **one** write through the single activation writer
//! ([`prepare_ledger_write`] for the ledger target; the canonical lossless
//! document writer for the upgrade-only legacy surface, exactly like
//! [`crate::plan::commit_plan`]). A refused or failed plan writes nothing.
//!
//! Replace semantics
//! -----------------
//! The preset governs every key in [`governed_keys`] except
//! `mission_type_activations` when the preset does not list it (an absent
//! `mission_type_activations` fails mission creation closed, so a preset that
//! does not mention mission types leaves that key untouched, spec FR-001):
//!
//! * a per-kind `activated_<plural>` key the preset lists is written as listed,
//!   followed by the org's `required_<plural>` ids it does not already hold
//!   (first-seen order across org packs);
//! * a per-kind key or `activated_kinds` the preset leaves out is **removed**:
//!   an absent key is unrestricted, which already admits every org-required id;
//! * `activated_kinds`, when listed, is written as listed plus the plural of
//!   every kind for which the org requires ids and which the list leaves out, so
//!   the kind gate never switches off an org requirement;
//! * `mission_type_activations`, when listed, is written as listed.
//!
//! `activated_skills` and `activated_glossary_packs` are never read or written
//! here: each has its own absence contract.
//!
//! Customised keys (OD-6)
//! ----------------------
//! A governed key that would change is **customised** when it is present and its
//! current value differs both from the value the preset leaves and from the value
//! the built-in `default` preset leaves (per-kind keys and `activated_kinds`:
//! absent; `mission_type_activations`: the `default` preset's list). Lists are
//! compared as sets for this test. Applying a plan with a customised change is
//! refused unless forced ([`PresetApplicationError::WouldOverwrite`], carrying the
//! per-key diff), so a freshly initialised project needs no `--force`.
//!
//! Id resolution
//! -------------
//! Every id a preset lists resolves against the whole offering (built-in, every
//! org pack, the project layer) through the one [`IdentityIndex`]: a config stem,
//! a declared `id:` of that kind, or a `<kind>:<id>` URN. Only listed kinds are
//! read, so the `default` preset reads no artifact. Anti-pattern ids resolve
//! against the anti-pattern nodes of the merged DRG (they have no artifact file).
//! Directive ids are compared through the identity index's declared-id key, so a
//! stem and a `DIRECTIVE_NNN` spelling of one artifact are equal. Mission types
//! resolve against the layered mission-type roster. The preset name is never
//! persisted.
//!
//! Contract mirror (Python reference, never invoked at runtime):
//! `src/charter/activation/preset_application.py`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;
use sk_charter::bundle_restamp::{
    RestampError, RestampLockFailure, RestampOutcome, restamp_bundle_hash,
};
use sk_charter::presets::{
    ACTIVATED_KINDS_KEY, ActivationPreset, DEFAULT_PRESET_NAME, MISSION_TYPE_ACTIVATIONS_KEY,
    OfferingPack, PresetFormatError, PresetLoadError, PresetNotFoundError, governed_kinds,
    list_offering_packs, load_preset, preset_activation_key, preset_activation_keys,
};

use crate::error::DoctrineError;
use crate::graph::NodeKind;
use crate::identity::{IdentityIndex, scan_all};
use crate::ledger::{
    ActivationSource, DenyListChoice, activatable_kinds, deny_key, effective_deny,
    load_activation_ledger, prepare_ledger_write, refuse_unmigrated_deny_write,
    resolve_activation_source,
};
use crate::lock::acquire_synthesis_lock;
use crate::org_charter::{
    RequiredFloor, built_in_search, normalize_packs_root, packs_root_override, required_floor,
};
use crate::plan::{ActivationObservation, activation_write_target, deny_delta};
use crate::yaml::{DEFAULT_LOCK_TIMEOUT, PreparedWrite, YamlDocument};

/// `reasons` key naming the org pack registry when it cannot be read.
pub const ORG_PACKS_KEY: &str = "charter_packs.org.packs";

/// Every key a preset can govern, in report order.
///
/// The per-kind keys (derived from [`ArtifactKind`]), the kind gate, then the
/// mission-type ledger.
pub fn governed_keys() -> Vec<String> {
    let mut keys = preset_activation_keys();
    keys.push(ACTIVATED_KINDS_KEY.to_owned());
    keys.push(MISSION_TYPE_ACTIVATIONS_KEY.to_owned());
    keys
}

/// The [`ArtifactKind`] a per-kind activation key names.
fn kind_for_key(key: &str) -> Option<ArtifactKind> {
    governed_kinds()
        .into_iter()
        .find(|kind| preset_activation_key(*kind) == key)
}

/// Whether `key` is a `deactivated_<plural>` exclusion key.
fn is_deny_key(key: &str) -> bool {
    activatable_kinds()
        .into_iter()
        .filter_map(|kind| deny_key(kind).ok())
        .any(|candidate| candidate == key)
}

fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            other => out.push(other),
        }
    }
    out.push('\'');
    out
}

fn listing<S: AsRef<str>>(names: &[S]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names
            .iter()
            .map(|name| name.as_ref())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// `absent` / `[a, b]` — how the reference renders a target value.
fn render_value(value: Option<&Vec<String>>) -> String {
    match value {
        None => "absent".to_owned(),
        Some(ids) => format!("[{}]", ids.join(", ")),
    }
}

// ---------------------------------------------------------------------------
// Errors (one base, one stable `code` each; the CLI renders them in one place)
// ---------------------------------------------------------------------------

/// Base of the coded preset-application errors (`contracts/errors.md`).
#[derive(Debug)]
pub enum PresetApplicationError {
    /// No pack of that name is in the project's offering.
    PackNotFound {
        /// The requested pack name.
        pack: String,
        /// The packs the project does offer.
        available: Vec<String>,
    },
    /// The pack ships no preset of that name (the `project` pack ships none).
    PresetNotFound {
        /// The pack name.
        pack: String,
        /// The requested preset name.
        preset: String,
        /// The presets the pack does ship.
        available: Vec<String>,
    },
    /// An id the preset lists resolves nowhere in the offering (or cannot be checked).
    IdUnresolved {
        /// The preset file.
        preset_file: PathBuf,
        /// Key -> the ids that resolve nowhere.
        unresolved: BTreeMap<String, Vec<String>>,
        /// Key -> why the ids cannot be checked.
        reasons: BTreeMap<String, String>,
    },
    /// A customised governed key would change and `--force` was not given (OD-6).
    WouldOverwrite(Box<PresetPlan>),
    /// The preset file is malformed.
    Format(PresetFormatError),
    /// The activation write landed but the synthesis manifest could not be
    /// re-stamped (the change is saved; the stamp is not).
    Restamp(RestampError),
    /// The project's active charter configuration cannot be read or written.
    Config(String),
    /// Any other domain failure (identity collision, YAML, lock, IO).
    Doctrine(DoctrineError),
}

impl PresetApplicationError {
    /// The stable machine code of this refusal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::PackNotFound { .. } => "PACK_NOT_FOUND",
            Self::PresetNotFound { .. } => "PRESET_NOT_FOUND",
            Self::IdUnresolved { .. } => "PRESET_ID_UNRESOLVED",
            Self::WouldOverwrite(_) => "PRESET_WOULD_OVERWRITE",
            Self::Format(_) => "PRESET_FORMAT_INVALID",
            Self::Restamp(_) => "RESTAMP_FAILED",
            Self::Config(_) => "ACTIVE_CHARTER_CONFIG_INVALID",
            Self::Doctrine(error) => error.reference_code().unwrap_or("ACTIVATION_FAILED"),
        }
    }

    /// Lines printed under the `Error (<CODE>): <message>` line.
    pub fn detail_lines(&self) -> Vec<String> {
        match self {
            Self::IdUnresolved {
                unresolved,
                reasons,
                ..
            } => {
                let mut lines: Vec<String> = Vec::new();
                for (key, ids) in unresolved {
                    if reasons.contains_key(key) {
                        continue;
                    }
                    lines.extend(ids.iter().map(|id| format!("  {key}: {id}")));
                }
                lines.extend(reasons.iter().map(|(key, why)| format!("  {key}: {why}")));
                lines
            }
            Self::WouldOverwrite(plan) => plan
                .changes
                .iter()
                .map(|change| {
                    format!(
                        "  {}: {} -> {}",
                        change.key,
                        render_value(change.before.as_ref()),
                        render_value(change.after.as_ref())
                    )
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The requested pack name, when the refusal names one.
    pub fn pack(&self) -> Option<&str> {
        match self {
            Self::PackNotFound { pack, .. } | Self::PresetNotFound { pack, .. } => Some(pack),
            Self::WouldOverwrite(plan) => Some(&plan.pack),
            _ => None,
        }
    }

    /// The requested preset name, when the refusal names one.
    pub fn preset(&self) -> Option<&str> {
        match self {
            Self::PresetNotFound { preset, .. } => Some(preset),
            Self::WouldOverwrite(plan) => Some(&plan.preset),
            _ => None,
        }
    }

    /// The names the refused lookup did find.
    pub fn available(&self) -> &[String] {
        match self {
            Self::PackNotFound { available, .. } | Self::PresetNotFound { available, .. } => {
                available
            }
            _ => &[],
        }
    }

    /// The per-key unresolved ids of an id-resolution refusal.
    pub fn unresolved(&self) -> Option<&BTreeMap<String, Vec<String>>> {
        match self {
            Self::IdUnresolved { unresolved, .. } => Some(unresolved),
            _ => None,
        }
    }

    /// The per-key "cannot be checked" reasons of an id-resolution refusal.
    pub fn reasons(&self) -> Option<&BTreeMap<String, String>> {
        match self {
            Self::IdUnresolved { reasons, .. } => Some(reasons),
            _ => None,
        }
    }

    /// The preset file an id-resolution or format refusal names.
    pub fn preset_file(&self) -> Option<&Path> {
        match self {
            Self::IdUnresolved { preset_file, .. } => Some(preset_file),
            Self::Format(error) => Some(&error.path),
            _ => None,
        }
    }

    /// The plan a forced-overwrite refusal carries.
    pub fn plan(&self) -> Option<&PresetPlan> {
        match self {
            Self::WouldOverwrite(plan) => Some(plan),
            _ => None,
        }
    }

    /// The customised keys a forced-overwrite refusal names.
    pub fn customised_changes(&self) -> &[String] {
        match self {
            Self::WouldOverwrite(plan) => &plan.customised_changes,
            _ => &[],
        }
    }
}

impl fmt::Display for PresetApplicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PackNotFound { pack, available } => write!(
                formatter,
                "pack {} not found. Available packs: {}",
                quoted(pack),
                listing(available)
            ),
            Self::PresetNotFound {
                pack,
                preset,
                available,
            } => write!(
                formatter,
                "preset {} not found in pack {}. Presets of {}: {}",
                quoted(preset),
                quoted(pack),
                quoted(pack),
                listing(available)
            ),
            Self::IdUnresolved {
                preset_file,
                unresolved,
                reasons,
            } => {
                let nowhere = unresolved
                    .iter()
                    .any(|(key, ids)| !ids.is_empty() && !reasons.contains_key(key));
                let mut named: Vec<String> = unresolved
                    .iter()
                    .filter(|(key, ids)| !ids.is_empty() && !reasons.contains_key(*key))
                    .map(|(key, ids)| format!("{key}: {}", ids.join(", ")))
                    .collect();
                named.extend(reasons.iter().map(|(key, why)| format!("{key}: {why}")));
                named.sort();
                named.dedup();
                let headline = if nowhere {
                    "lists ids that resolve nowhere in the offering"
                } else {
                    "lists ids that cannot be checked against the offering"
                };
                write!(
                    formatter,
                    "{} {headline} ({})",
                    preset_file.display(),
                    named.join("; ")
                )
            }
            Self::WouldOverwrite(plan) => write!(
                formatter,
                "preset {} of pack {} would change customised key(s) {} in {}; --force applies it",
                quoted(&plan.preset),
                quoted(&plan.pack),
                plan.customised_changes.join(", "),
                plan.target_file.display()
            ),
            Self::Format(error) => error.fmt(formatter),
            Self::Restamp(error) => write!(
                formatter,
                "the preset was applied, but the synthesis manifest's bundle hash was not refreshed: {error}"
            ),
            Self::Config(message) => formatter.write_str(message),
            Self::Doctrine(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PresetApplicationError {}

impl From<DoctrineError> for PresetApplicationError {
    fn from(error: DoctrineError) -> Self {
        Self::Doctrine(error)
    }
}

impl From<PresetFormatError> for PresetApplicationError {
    fn from(error: PresetFormatError) -> Self {
        Self::Format(error)
    }
}

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// One governed key whose value would change (`None` means absent).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresetChange {
    /// The governed key.
    pub key: String,
    /// The value the file holds now.
    pub before: Option<Vec<String>>,
    /// The value the preset leaves.
    pub after: Option<Vec<String>>,
}

/// The whole change applying a preset makes; computed before any write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresetPlan {
    /// The pack the preset belongs to.
    pub pack: String,
    /// The preset name.
    pub preset: String,
    /// The activation file the plan writes.
    pub target_file: PathBuf,
    /// Allow keys to write, with their post-state ids.
    pub written: BTreeMap<String, Vec<String>>,
    /// Keys to remove (an absent key is unrestricted).
    pub removed: Vec<String>,
    /// Every governed key whose value would change.
    pub changes: Vec<PresetChange>,
    /// The changed keys whose current value is hand-made.
    pub customised_changes: Vec<String>,
    /// Listed ids that are deactivated and therefore left out.
    pub denied_listed: BTreeMap<String, Vec<String>>,
    /// With `--force`: the listed ids written back over the deactivation.
    pub forced_written: BTreeMap<String, Vec<String>>,
    /// With `--force`: `deactivated_<plural>` -> the post-state list.
    pub forced_deny: BTreeMap<String, Vec<String>>,
    /// With `--force`: `deactivated_<plural>` -> the ids whose exclusion the
    /// PLAN read as cleared.
    ///
    /// This is the plan-time reading. [`apply_preset_plan`] re-reads the deny
    /// list under the run-wide lock (exactly like the reference's
    /// `_cleared_by_force`) and reports that delta in
    /// [`PresetOutcome::cleared`]; this field stays as the plan's own snapshot.
    pub forced_cleared: BTreeMap<String, Vec<String>>,
    /// The activation authority as the plan read it.
    observation: ActivationObservation,
}

impl PresetPlan {
    /// Whether applying the plan without `--force` changes nothing.
    pub fn is_noop(&self) -> bool {
        self.written.is_empty() && self.removed.is_empty()
    }

    /// The activation authority as the plan read it.
    pub fn observation(&self) -> &ActivationObservation {
        &self.observation
    }
}

/// What [`apply_preset_plan`] wrote (FR-009, FR-014).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PresetOutcome {
    /// The allow lists actually written.
    pub written: BTreeMap<String, Vec<String>>,
    /// Without `--force`: an allow key -> the deactivated ids the preset lists,
    /// left out of it and still excluded.
    pub kept_deactivated: BTreeMap<String, Vec<String>>,
    /// With `--force`: a `deactivated_<plural>` key -> the ids whose exclusion
    /// was cleared, re-read from the authority under the run-wide lock.
    pub cleared: BTreeMap<String, Vec<String>>,
    /// The manifest bundle-hash re-stamp this write closed with.
    ///
    /// `None` only when no write happened at all (a true no-op plan, or the
    /// forced-overwrite refusal): the reference does not re-stamp then either.
    pub restamp: Option<RestampOutcome>,
}

impl PresetOutcome {
    /// The one stderr warning line a lock the re-stamp could not take emits,
    /// or `None` when the re-stamp did not skip on a lock.
    #[must_use]
    pub fn restamp_warning(&self) -> Option<String> {
        self.restamp.as_ref().and_then(RestampOutcome::warning_line)
    }
}

// ---------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------

/// Return the offering pack named `pack_name`, or a refusal naming the available packs.
pub fn find_offering_pack(
    repo_root: &Path,
    pack_name: &str,
) -> Result<OfferingPack, PresetApplicationError> {
    let packs = list_offering_packs(repo_root, &sk_charter::Limits::default())
        .map_err(|error| PresetApplicationError::Config(error.to_string()))?;
    if let Some(pack) = packs.iter().find(|pack| pack.name == pack_name) {
        return Ok(pack.clone());
    }
    Err(PresetApplicationError::PackNotFound {
        pack: pack_name.to_owned(),
        available: packs.into_iter().map(|pack| pack.name).collect(),
    })
}

/// Load preset `preset_name` of `pack`, or a refusal naming the pack's presets.
///
/// The `project` pack ships no presets. A malformed preset file is a
/// [`PresetApplicationError::Format`].
pub fn load_pack_preset(
    pack: &OfferingPack,
    preset_name: &str,
) -> Result<ActivationPreset, PresetApplicationError> {
    if !pack.ships_presets() {
        return Err(PresetApplicationError::PresetNotFound {
            pack: pack.name.clone(),
            preset: preset_name.to_owned(),
            available: Vec::new(),
        });
    }
    match load_preset(&pack.root, preset_name) {
        Ok(preset) => Ok(preset),
        Err(PresetLoadError::NotFound(PresetNotFoundError { available, .. })) => {
            Err(PresetApplicationError::PresetNotFound {
                pack: pack.name.clone(),
                preset: preset_name.to_owned(),
                available,
            })
        }
        Err(PresetLoadError::Format(error)) => Err(error.into()),
    }
}

// ---------------------------------------------------------------------------
// Offering roots and id resolution
// ---------------------------------------------------------------------------

/// The offering's roots, resolved once per plan (built-in, every org pack, project).
#[derive(Clone, Debug)]
struct OfferingRoots {
    built_in: PathBuf,
    org_roots: Vec<PathBuf>,
    project: Option<PathBuf>,
}

impl OfferingRoots {
    fn as_service_roots(&self) -> crate::effective_set::OfferingRoots {
        crate::effective_set::OfferingRoots {
            built_in: Some(self.built_in.clone()),
            org_roots: self.org_roots.clone(),
            project: self.project.clone(),
        }
    }
}

/// The offering's roots for `repo_root`.
///
/// `strict` fails closed on a declared org pack that is missing or unreadable
/// (the effective-set seam's own precondition); otherwise only the readable
/// roots are returned. Both modes use the one `sk-charter` path authority and
/// the documented packs-root override.
fn resolve_offering_roots(repo_root: &Path, strict: bool) -> Result<OfferingRoots, DoctrineError> {
    if strict {
        let (built_in, org_roots, project) = crate::context::context_roots(repo_root)?;
        return Ok(OfferingRoots {
            built_in,
            org_roots,
            project,
        });
    }
    let config = sk_charter::load_project_config(repo_root, &sk_charter::Limits::default())?;
    let expansion = sk_charter::PathExpansion::from_env();
    let mut search = built_in_search(repo_root);
    if let Some(root) = packs_root_override() {
        search = search.with_packs_root_env(normalize_packs_root(root));
    }
    let roots = sk_charter::PackRoots::resolve_existing(
        repo_root,
        Some(&config),
        &expansion,
        Some(&search),
    )?;
    let built_in = roots
        .built_in
        .ok_or(DoctrineError::PackRootUnavailable { tier: "built-in" })?;
    Ok(OfferingRoots {
        built_in,
        org_roots: roots
            .organizations
            .into_iter()
            .map(|pack| pack.root)
            .collect(),
        project: roots.project,
    })
}

/// Whether the preset lists per-kind ids or a kind gate (the org union then applies).
fn lists_ids(preset: &ActivationPreset) -> bool {
    !preset.activations.is_empty() || preset.activated_kinds.is_some()
}

/// Comparison key of a listed id: its declared id (a stem and a declared id
/// spell one artifact, FR-017, #6406).
fn id_key(yaml_key: &str, token: &str, identity: Option<&IdentityIndex>) -> String {
    let kind = kind_for_key(yaml_key);
    match identity {
        None => IdentityIndex::unindexed_key(kind, token),
        Some(index) => index.lookup_key(kind, token, false),
    }
}

fn missing_ids(
    yaml_key: &str,
    listed: &[String],
    available: &[String],
    identity: Option<&IdentityIndex>,
) -> Vec<String> {
    let known: BTreeSet<String> = available
        .iter()
        .map(|token| id_key(yaml_key, token, identity))
        .collect();
    listed
        .iter()
        .filter(|token| !known.contains(&id_key(yaml_key, token, identity)))
        .cloned()
        .collect()
}

/// The anti-pattern node ids of the merged DRG (anti-patterns have no artifact file).
///
/// Composed from the public layer primitives the action-context bundle uses
/// (root graphs, org-shaped fragments, project overlay). Only paid when a
/// preset lists anti-pattern ids.
fn load_anti_pattern_ids(roots: &OfferingRoots) -> Result<BTreeSet<String>, DoctrineError> {
    let mut graph = crate::loader::load_graph_or_dir(&roots.built_in)?;
    let mut fragments = Vec::new();
    for (index, root) in roots.org_roots.iter().enumerate() {
        if crate::loader::has_graph_files(root) {
            let root_graph = crate::loader::load_graph_or_dir(root)?;
            graph = crate::loader::merge_layers(&graph, Some(&root_graph))?;
        }
        if root.join(crate::org::ORG_FRAGMENT).exists() {
            let name = root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            fragments.push(crate::org::load_org_pack(&name, root, index + 1)?);
        }
    }
    let project_graph = match &roots.project {
        Some(root) if crate::loader::has_graph_files(root) => {
            Some(crate::loader::load_graph_or_dir(root)?)
        }
        _ => None,
    };
    let merged = crate::merge::merge_three_layers(&graph, &fragments, project_graph.as_ref())?;
    Ok(merged
        .graph
        .nodes()
        .iter()
        .filter(|node| node.kind() == NodeKind::AntiPattern)
        .filter_map(|node| crate::graph::split_urn(node.urn()).map(|(_, id)| id.to_owned()))
        .collect())
}

/// The declared org roots, failing closed when the preset lists ids or a gate.
fn org_roots_or_refuse(
    repo_root: &Path,
    preset: &ActivationPreset,
) -> Result<OfferingRoots, PresetApplicationError> {
    let strict = lists_ids(preset);
    match resolve_offering_roots(repo_root, strict) {
        Ok(roots) => Ok(roots),
        Err(error) => {
            let mut reasons = BTreeMap::new();
            reasons.insert(ORG_PACKS_KEY.to_owned(), error.to_string());
            Err(PresetApplicationError::IdUnresolved {
                preset_file: preset.source.clone(),
                unresolved: BTreeMap::new(),
                reasons,
            })
        }
    }
}

/// Resolve every listed id; return the roots and the index it resolved against.
fn resolve_preset_ids(
    repo_root: &Path,
    preset: &ActivationPreset,
) -> Result<(OfferingRoots, IdentityIndex), PresetApplicationError> {
    let roots = org_roots_or_refuse(repo_root, preset)?;
    let mut identity =
        IdentityIndex::from_roots(&roots.built_in, &roots.org_roots, roots.project.as_deref());
    scan_all(&mut identity);
    let mut unresolved: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut reasons: BTreeMap<String, String> = BTreeMap::new();
    check_artifact_ids(&identity, &roots, preset, &mut unresolved, &mut reasons);
    check_mission_types(&roots, preset, &mut unresolved);
    unresolved.retain(|_, ids| !ids.is_empty());
    if !unresolved.is_empty() || !reasons.is_empty() {
        return Err(PresetApplicationError::IdUnresolved {
            preset_file: preset.source.clone(),
            unresolved,
            reasons,
        });
    }
    Ok((roots, identity))
}

fn check_artifact_ids(
    identity: &IdentityIndex,
    roots: &OfferingRoots,
    preset: &ActivationPreset,
    unresolved: &mut BTreeMap<String, Vec<String>>,
    reasons: &mut BTreeMap<String, String>,
) {
    for kind in preset.listed_kinds() {
        let key = preset_activation_key(kind);
        let listed = preset.activations.get(&key).cloned().unwrap_or_default();
        if kind == ArtifactKind::AntiPattern {
            if listed.is_empty() {
                continue;
            }
            match load_anti_pattern_ids(roots) {
                Ok(known) => {
                    let known: Vec<String> = known.into_iter().collect();
                    unresolved.insert(
                        key.clone(),
                        missing_ids(&key, &listed, &known, Some(identity)),
                    );
                }
                Err(error) => {
                    reasons.insert(
                        key,
                        format!("the DRG cannot be loaded to check anti-pattern ids: {error}"),
                    );
                }
            }
            continue;
        }
        unresolved.insert(
            key.clone(),
            listed
                .iter()
                .filter(|token| !identity.knows(kind, token))
                .cloned()
                .collect(),
        );
    }
}

fn check_mission_types(
    roots: &OfferingRoots,
    preset: &ActivationPreset,
    unresolved: &mut BTreeMap<String, Vec<String>>,
) {
    let Some(listed) = preset.mission_type_activations.as_ref() else {
        return;
    };
    if listed.is_empty() {
        return;
    }
    let available: Vec<String> = crate::offering::mission_type_fallback(&roots.as_service_roots())
        .into_iter()
        .filter_map(|id| id.to_scalar_string().ok())
        .collect();
    unresolved.insert(
        MISSION_TYPE_ACTIVATIONS_KEY.to_owned(),
        missing_ids(MISSION_TYPE_ACTIVATIONS_KEY, listed, &available, None),
    );
}

// ---------------------------------------------------------------------------
// Target state
// ---------------------------------------------------------------------------

/// `required_<plural>` ids per plural (non-empty lists only); none when the
/// preset lists no ids.
fn org_required(floor: &RequiredFloor) -> BTreeMap<String, Vec<String>> {
    floor
        .tokens
        .iter()
        .filter(|(_, ids)| !ids.is_empty())
        .map(|(kind, ids)| (kind.plural().to_owned(), ids.clone()))
        .collect()
}

/// The org required floor, or the coded refusal the reference raises.
fn floor_or_refuse(
    repo_root: &Path,
    preset: &ActivationPreset,
    identity: &IdentityIndex,
) -> Result<RequiredFloor, PresetApplicationError> {
    match required_floor(repo_root, identity) {
        Ok(floor) => Ok(floor),
        Err(error) => {
            let mut reasons = BTreeMap::new();
            reasons.insert(
                ORG_PACKS_KEY.to_owned(),
                format!("the org charter cannot be read: {error}"),
            );
            Err(PresetApplicationError::IdUnresolved {
                preset_file: preset.source.clone(),
                unresolved: BTreeMap::new(),
                reasons,
            })
        }
    }
}

/// `listed` followed by `extra` ids not already present (first-seen order).
fn union_ids(
    yaml_key: &str,
    listed: &[String],
    extra: &[String],
    identity: Option<&IdentityIndex>,
) -> Vec<String> {
    let mut result: Vec<String> = listed.to_vec();
    let mut seen: BTreeSet<String> = result
        .iter()
        .map(|token| id_key(yaml_key, token, identity))
        .collect();
    for token in extra {
        if seen.insert(id_key(yaml_key, token, identity)) {
            result.push(token.clone());
        }
    }
    result
}

/// First occurrence kept, in order.
fn dedupe(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for id in ids {
        if seen.insert(id.clone()) {
            out.push(id);
        }
    }
    out
}

/// `ids` as activation stems, first occurrence kept (#6527, FR-013); without an
/// index, as given.
///
/// A colliding declared id is refused through the caller's
/// [`DoctrineError::ActivationIdCollision`] mapping (Decision
/// `file_name_collision`).
fn stored_spelling(
    yaml_key: &str,
    ids: &[String],
    identity: Option<&IdentityIndex>,
) -> Result<Vec<String>, DoctrineError> {
    let (Some(identity), Some(kind)) = (identity, kind_for_key(yaml_key)) else {
        return Ok(ids.to_vec());
    };
    let mut stems = Vec::with_capacity(ids.len());
    for id in ids {
        stems.push(identity.stored_spelling(kind, id)?);
    }
    Ok(dedupe(stems))
}

/// The value every key this preset governs takes (`None` = absent); listed and
/// org-required ids are stored as stems.
fn target_state(
    preset: &ActivationPreset,
    required: &BTreeMap<String, Vec<String>>,
    identity: Option<&IdentityIndex>,
) -> Result<BTreeMap<String, Option<Vec<String>>>, PresetApplicationError> {
    let mut target: BTreeMap<String, Option<Vec<String>>> = BTreeMap::new();
    for key in preset_activation_keys() {
        match preset.activations.get(&key) {
            None => {
                target.insert(key, None);
            }
            Some(listed) => {
                let extra = required
                    .get(key.strip_prefix("activated_").unwrap_or(&key))
                    .cloned()
                    .unwrap_or_default();
                let union = union_ids(&key, listed, &extra, identity);
                let stems = stored_spelling(&key, &union, identity).map_err(|error| {
                    collision_refusal(preset, &key, &error).unwrap_or(error.into())
                })?;
                target.insert(key, Some(stems));
            }
        }
    }
    let gate = match &preset.activated_kinds {
        None => None,
        Some(gate) => {
            let extra: Vec<String> = required.keys().cloned().collect();
            Some(union_ids(ACTIVATED_KINDS_KEY, gate, &extra, None))
        }
    };
    target.insert(ACTIVATED_KINDS_KEY.to_owned(), gate);
    if let Some(mission_types) = &preset.mission_type_activations {
        target.insert(
            MISSION_TYPE_ACTIVATIONS_KEY.to_owned(),
            Some(mission_types.clone()),
        );
    }
    Ok(target)
}

/// Map an id collision on a per-kind key to the reference's coded refusal.
fn collision_refusal(
    preset: &ActivationPreset,
    key: &str,
    error: &DoctrineError,
) -> Option<PresetApplicationError> {
    let DoctrineError::ActivationIdCollision {
        artifact_id,
        declarations,
        ..
    } = error
    else {
        return None;
    };
    let mut unresolved = BTreeMap::new();
    unresolved.insert(key.to_owned(), vec![artifact_id.clone()]);
    let mut reasons = BTreeMap::new();
    reasons.insert(key.to_owned(), format!("{error} ({})", declarations.len()));
    Some(PresetApplicationError::IdUnresolved {
        preset_file: preset.source.clone(),
        unresolved,
        reasons,
    })
}

/// What the built-in `default` preset leaves for the keys it could govern.
fn default_values(repo_root: &Path) -> BTreeMap<String, Option<Vec<String>>> {
    let mut values = BTreeMap::new();
    let Ok(built_in) = built_in_search(repo_root).find_root() else {
        return values;
    };
    let Ok(default) = load_preset(&built_in, DEFAULT_PRESET_NAME) else {
        return values;
    };
    values.insert(
        MISSION_TYPE_ACTIVATIONS_KEY.to_owned(),
        default.mission_type_activations,
    );
    values
}

/// Set equality for lists (`None` is absent, which equals only absent).
fn same(left: &[String], right: Option<&Vec<String>>) -> bool {
    let Some(right) = right else {
        return false;
    };
    let mut left_sorted: Vec<&String> = left.iter().collect();
    let mut right_sorted: Vec<&String> = right.iter().collect();
    left_sorted.sort();
    right_sorted.sort();
    left_sorted == right_sorted
}

/// Whether `before` is a hand-made list; a stored list that only spells the
/// post-state differently is not (#6527).
fn customised(
    key: &str,
    before: &Option<Vec<String>>,
    after: &Option<Vec<String>>,
    defaults: &BTreeMap<String, Option<Vec<String>>>,
    identity: Option<&IdentityIndex>,
) -> bool {
    let Some(before_value) = before else {
        return false;
    };
    let normalised: Vec<String> = match (identity, kind_for_key(key)) {
        (Some(index), Some(kind)) => dedupe(
            before_value
                .iter()
                .map(|item| index.stem(kind, item).unwrap_or_else(|| item.clone())),
        ),
        _ => before_value.clone(),
    };
    !same(&normalised, after.as_ref())
        && !same(&normalised, defaults.get(key).and_then(Option::as_ref))
}

/// The preset-listed ids that are deactivated, and the post-`--force` lists.
#[derive(Default)]
struct DenyPartition {
    denied_listed: BTreeMap<String, Vec<String>>,
    forced_written: BTreeMap<String, Vec<String>>,
    forced_deny: BTreeMap<String, Vec<String>>,
    forced_cleared: BTreeMap<String, Vec<String>>,
}

/// `stored` less every entry naming one of `listed` (stem or declared id).
fn cleared_deny(
    stored: &[String],
    listed: &[String],
    kind: ArtifactKind,
    identity: &IdentityIndex,
) -> Vec<String> {
    let listed_stems: BTreeSet<String> = listed
        .iter()
        .map(|item| identity.stem(kind, item).unwrap_or_else(|| item.clone()))
        .collect();
    stored
        .iter()
        .filter(|item| {
            let named = listed.iter().any(|hit| hit == *item)
                || listed_stems
                    .contains(&identity.stem(kind, item).unwrap_or_else(|| (*item).clone()));
            !named
        })
        .cloned()
        .collect()
}

/// Take the effective-denied ids out of every listed allow list (FR-009);
/// record what `--force` writes instead.
///
/// The deny keys are never governed (a preset names no deny key); only
/// `--force` clears the exclusions of the ids the preset lists (FR-014).
fn deny_partition(
    target: &mut BTreeMap<String, Option<Vec<String>>>,
    document: &YamlDocument,
    ledger_deny: &BTreeMap<ArtifactKind, Vec<String>>,
    floor: &BTreeMap<ArtifactKind, Vec<String>>,
    identity: &IdentityIndex,
) -> Result<DenyPartition, DoctrineError> {
    let mut partition = DenyPartition::default();
    let keys: Vec<String> = target.keys().cloned().collect();
    for key in keys {
        let Some(kind) = kind_for_key(&key) else {
            continue;
        };
        let Some(listed) = target.get(&key).cloned().flatten() else {
            continue;
        };
        let denied = effective_deny(
            kind,
            ledger_deny.get(&kind).cloned().unwrap_or_default(),
            floor.get(&kind).cloned().unwrap_or_default(),
            identity,
        );
        let hit: Vec<String> = listed
            .iter()
            .filter(|item| !denied.admits(item))
            .cloned()
            .collect();
        if hit.is_empty() {
            continue;
        }
        let exclusion_key = deny_key(kind)?;
        let stored = document
            .get_string_list(&exclusion_key)?
            .unwrap_or_default();
        let remaining = cleared_deny(&stored, &hit, kind, identity);
        let (_, cleared) = deny_delta(Some(stored.as_slice()), Some(remaining.as_slice()));
        partition.denied_listed.insert(key.clone(), hit.clone());
        partition.forced_written.insert(key.clone(), listed.clone());
        partition
            .forced_deny
            .insert(exclusion_key.clone(), remaining);
        partition.forced_cleared.insert(exclusion_key, cleared);
        target.insert(
            key,
            Some(
                listed
                    .into_iter()
                    .filter(|item| !hit.iter().any(|entry| entry == item))
                    .collect(),
            ),
        );
    }
    Ok(partition)
}

// ---------------------------------------------------------------------------
// Plan
// ---------------------------------------------------------------------------

/// Compute the change applying `preset_name` of `pack_name` makes (pure read).
///
/// Refuses with a coded [`PresetApplicationError`] for an unknown pack, an
/// unknown preset, a malformed preset file, an unresolvable listed id, or an
/// unreadable active charter configuration. Never writes.
pub fn plan_preset_application(
    repo_root: &Path,
    pack_name: &str,
    preset_name: &str,
) -> Result<PresetPlan, PresetApplicationError> {
    let pack = find_offering_pack(repo_root, pack_name)?;
    let preset = load_pack_preset(&pack, preset_name)?;
    let (_roots, index) = resolve_preset_ids(repo_root, &preset)?;
    let identity = if lists_ids(&preset) {
        Some(&index)
    } else {
        None
    };
    let floor = if lists_ids(&preset) {
        Some(floor_or_refuse(repo_root, &preset, &index)?)
    } else {
        None
    };
    let required = floor.as_ref().map(org_required).unwrap_or_default();
    let mut target = target_state(&preset, &required, identity)?;
    let observation = ActivationObservation::observe(repo_root)?;
    let target_file = observation.target().to_path_buf();
    let document = YamlDocument::open(&target_file)?;
    let ledger = load_activation_ledger(repo_root)?;
    let floor_tokens = floor.as_ref().map(|floor| &floor.tokens);
    let deny = match identity {
        None => DenyPartition::default(),
        Some(identity) => deny_partition(
            &mut target,
            &document,
            &ledger.deny,
            floor_tokens.unwrap_or(&BTreeMap::new()),
            identity,
        )?,
    };
    let defaults = default_values(repo_root);
    let mut written: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut removed: Vec<String> = Vec::new();
    let mut changes: Vec<PresetChange> = Vec::new();
    let mut customised_changes: Vec<String> = Vec::new();
    for key in governed_keys() {
        let Some(after) = target.get(&key) else {
            continue;
        };
        let before = document.get_string_list(&key)?;
        if &before == after {
            continue;
        }
        changes.push(PresetChange {
            key: key.clone(),
            before: before.clone(),
            after: after.clone(),
        });
        match after {
            None => removed.push(key.clone()),
            Some(ids) => {
                written.insert(key.clone(), ids.clone());
            }
        }
        if customised(&key, &before, after, &defaults, identity) {
            customised_changes.push(key);
        }
    }
    Ok(PresetPlan {
        pack: pack.name,
        preset: preset.name,
        target_file,
        written,
        removed,
        changes,
        customised_changes,
        denied_listed: deny.denied_listed,
        forced_written: deny.forced_written,
        forced_deny: deny.forced_deny,
        forced_cleared: deny.forced_cleared,
        observation,
    })
}

// ---------------------------------------------------------------------------
// Apply
// ---------------------------------------------------------------------------

/// Prepare the one activation write for a values/remove/deny-change triple.
///
/// The ledger target goes through [`prepare_ledger_write`] (the canonical
/// activation writer, which owns the deny-change guard and the unknown
/// `deactivated_*` key preservation). The upgrade-only legacy surface writes
/// the same lossless document through [`YamlDocument`] and refuses a deny
/// change, exactly like [`crate::plan::commit_plan`].
fn prepare_preset_write(
    repo_root: &Path,
    values: &BTreeMap<String, Option<Vec<String>>>,
    remove: &[String],
    deny_change: Option<DenyListChoice>,
) -> Result<PreparedWrite, DoctrineError> {
    let resolved = resolve_activation_source(repo_root)?;
    let target_is_ledger = resolved.source() == ActivationSource::Ledger || resolved.ledger_exists;
    if target_is_ledger {
        return prepare_ledger_write(repo_root, values, remove, deny_change);
    }
    let deny_keys: Vec<String> = values
        .keys()
        .chain(remove.iter())
        .filter(|key| is_deny_key(key))
        .cloned()
        .collect();
    if deny_change.is_some() || !deny_keys.is_empty() {
        refuse_unmigrated_deny_write(repo_root)?;
        return Err(DoctrineError::UnmigratedDenyWrite {
            keys: deny_keys.join(", "),
            files: repo_root.join(".kittify/config.yaml").display().to_string(),
        });
    }
    let target = activation_write_target(repo_root, &resolved)?;
    let mut document = YamlDocument::open(&target)?;
    for (key, value) in values {
        match value {
            Some(ids) => document.set_string_list(key, ids),
            None => document.remove(key),
        }
    }
    for key in remove {
        document.remove(key);
    }
    let desired = document.to_text().into_bytes();
    PreparedWrite::prepare(&target, desired).map(|write| write.with_synthesis_lock(repo_root))
}

/// The `--force` write: listed deactivated ids written back, their exclusions
/// cleared (an empty remainder removes the deny key).
fn forced_values(plan: &PresetPlan) -> (BTreeMap<String, Option<Vec<String>>>, Vec<String>) {
    let mut values: BTreeMap<String, Option<Vec<String>>> = plan
        .written
        .iter()
        .map(|(key, ids)| (key.clone(), Some(ids.clone())))
        .collect();
    for (key, ids) in &plan.forced_written {
        values.insert(key.clone(), Some(ids.clone()));
    }
    let mut removed = plan.removed.clone();
    for (key, remaining) in &plan.forced_deny {
        if remaining.is_empty() {
            values.remove(key);
            removed.push(key.clone());
        } else {
            values.insert(key.clone(), Some(remaining.clone()));
        }
    }
    (values, removed)
}

/// How the write rechecks the activation authority it prepared against.
#[derive(Clone, Copy, Debug)]
enum Precondition<'a> {
    /// Prepare the write from the authority's CURRENT bytes at apply time — the
    /// reference's policy. The plan supplies the key values only; the writer's
    /// own prepare/apply recheck still refuses a change in the window between
    /// them.
    PreparedAtApply,
    /// The prior Native policy: additionally refuse when the authority no longer
    /// holds the exact bytes the *plan* observed. Strictly narrower than the
    /// reference (it refuses a changed file the reference would re-read and
    /// apply), so it stays a separately named capability
    /// ([`apply_preset_plan_observed`]).
    PlanObservation(Option<&'a [u8]>),
}

/// Perform `plan` as the reference does: one write through the single
/// activation writer, then the manifest bundle-hash re-stamp.
///
/// The whole read-modify-write runs under the run-wide charter synthesis lock
/// (a preset write is one read-modify-write of the activation config, so a
/// synthesis run appending between the read and the write would otherwise lose
/// one of the two changes), and the write is prepared from the authority's
/// *current* bytes at apply time, exactly like
/// `charter.activation.charter_yaml_io.prepare_yaml_write` inside the
/// reference's `apply_preset_plan`. A file another writer changed between plan
/// and apply is therefore re-read and kept, not refused (measured against the
/// pinned reference: a comment added after the plan survives the apply).
///
/// Refuses with [`PresetApplicationError::WouldOverwrite`] (writing nothing and
/// without taking the lock) when the plan changes a customised key and `force`
/// is false. A no-op plan writes nothing and re-stamps nothing. Without
/// `force`, the deactivated ids the preset lists stay excluded and are left out
/// ([`PresetOutcome::kept_deactivated`]); with `force` they are written and
/// their exclusions cleared with [`DenyListChoice::PresetForce`], with the
/// `cleared` report re-read from the authority under the lock.
///
/// The manifest re-stamp cannot abort a change that is already saved: a lock it
/// cannot take yields [`RestampOutcome::Skipped`] and the one reference warning
/// line ([`PresetOutcome::restamp_warning`]).
pub fn apply_preset_plan(
    repo_root: &Path,
    plan: &PresetPlan,
    force: bool,
) -> Result<PresetOutcome, PresetApplicationError> {
    apply_preset_plan_with(repo_root, plan, force, Precondition::PreparedAtApply)
}

/// Perform `plan` under the stricter, Native-only plan-observation preconditions.
///
/// Identical to [`apply_preset_plan`] except that the write also refuses with
/// `precondition_changed` when the activation authority no longer holds the
/// exact bytes `plan_preset_application` observed. The reference has no such
/// check, so this is a deliberately separate capability, never the Source
/// composition.
pub fn apply_preset_plan_observed(
    repo_root: &Path,
    plan: &PresetPlan,
    force: bool,
) -> Result<PresetOutcome, PresetApplicationError> {
    apply_preset_plan_with(
        repo_root,
        plan,
        force,
        Precondition::PlanObservation(plan.observation.before()),
    )
}

fn apply_preset_plan_with(
    repo_root: &Path,
    plan: &PresetPlan,
    force: bool,
    precondition: Precondition<'_>,
) -> Result<PresetOutcome, PresetApplicationError> {
    // The refusal writes nothing, so it needs no lock (and must not wait on one).
    if !plan.customised_changes.is_empty() && !force {
        return Err(PresetApplicationError::WouldOverwrite(Box::new(
            plan.clone(),
        )));
    }
    let clears_deny = force && !plan.forced_deny.is_empty();
    // The run-wide lock covers the read-modify-write AND the re-stamp, exactly
    // like the reference's `with synthesis_lock(...)` in `apply_preset_plan`.
    let lock = acquire_synthesis_lock(repo_root, DEFAULT_LOCK_TIMEOUT)?;
    if !clears_deny && plan.is_noop() {
        drop(lock);
        return Ok(PresetOutcome {
            written: plan.written.clone(),
            kept_deactivated: plan.denied_listed.clone(),
            cleared: BTreeMap::new(),
            restamp: None,
        });
    }
    let (values, removed) = if clears_deny {
        forced_values(plan)
    } else {
        (
            plan.written
                .iter()
                .map(|(key, ids)| (key.clone(), Some(ids.clone())))
                .collect::<BTreeMap<String, Option<Vec<String>>>>(),
            plan.removed.clone(),
        )
    };
    let cleared = if clears_deny {
        cleared_from_current_deny(plan)?
    } else {
        BTreeMap::new()
    };
    let deny_change = if clears_deny {
        Some(DenyListChoice::PresetForce)
    } else {
        None
    };
    let prepared = prepare_preset_write(repo_root, &values, &removed, deny_change)?;
    if prepared.target() != plan.target_file.as_path() {
        return Err(DoctrineError::PreconditionChanged {
            path: plan.target_file.clone(),
        }
        .into());
    }
    if values.is_empty() && removed.is_empty() {
        drop(prepared);
    } else {
        match precondition {
            Precondition::PreparedAtApply => {
                prepared.apply(DEFAULT_LOCK_TIMEOUT)?;
            }
            Precondition::PlanObservation(observed) => {
                prepared.apply_observed(observed, DEFAULT_LOCK_TIMEOUT)?;
            }
        }
    }
    let restamp = restamp_manifest(repo_root)?;
    drop(lock);
    if clears_deny {
        return Ok(PresetOutcome {
            written: values
                .into_iter()
                .filter(|(key, _)| {
                    plan.written.contains_key(key) || plan.forced_written.contains_key(key)
                })
                .filter_map(|(key, ids)| ids.map(|ids| (key, ids)))
                .collect(),
            kept_deactivated: BTreeMap::new(),
            cleared,
            restamp: Some(restamp),
        });
    }
    Ok(PresetOutcome {
        written: plan.written.clone(),
        kept_deactivated: plan.denied_listed.clone(),
        cleared: BTreeMap::new(),
        restamp: Some(restamp),
    })
}

/// The ids `--force` takes off the CURRENT `deactivated_<plural>` lists.
///
/// The reference re-reads the authority under the run-wide lock
/// (`_read_target` + `_cleared_by_force`) instead of reporting the plan-time
/// delta, so a concurrent exclusion edit is reflected in the report.
fn cleared_from_current_deny(
    plan: &PresetPlan,
) -> Result<BTreeMap<String, Vec<String>>, DoctrineError> {
    let document = YamlDocument::open(&plan.target_file)?;
    let mut cleared: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, remaining) in &plan.forced_deny {
        let stored = document.get_string_list(key)?;
        let (_, taken) = deny_delta(stored.as_deref(), Some(remaining.as_slice()));
        if !taken.is_empty() {
            cleared.insert(key.clone(), taken);
        }
    }
    Ok(cleared)
}

/// The manifest bundle-hash re-stamp that closes every published activation write.
///
/// The run-wide lock is already held by the caller, and the canonical owner is
/// re-entrant, so this nests instead of opening a second lock family.
fn restamp_manifest(repo_root: &Path) -> Result<RestampOutcome, PresetApplicationError> {
    restamp_bundle_hash(repo_root, |root| {
        acquire_synthesis_lock(root, DEFAULT_LOCK_TIMEOUT).map_err(|error| match error {
            DoctrineError::SynthesisLockHeld { path, .. } => {
                RestampLockFailure::Held { lock_path: path }
            }
            DoctrineError::SynthesisLockUnavailable { .. } => RestampLockFailure::Unavailable,
            other => RestampLockFailure::Other {
                detail: other.to_string(),
            },
        })
    })
    .map_err(PresetApplicationError::Restamp)
}
