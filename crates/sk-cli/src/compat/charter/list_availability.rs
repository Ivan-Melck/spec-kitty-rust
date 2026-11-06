//! `charter list` composition: the list-SPECIFIC lenient roster, the
//! reference's availability rows, its required-floor status cells and the
//! Source-shaped payload.
//!
//! Mirrors `specify_cli/cli/commands/charter/list_cmd.py` (the `list_cmd`
//! callback and its `_template_tier_roots` / `_available_over_chain` /
//! `_required_not_in_force_*` / `_required_in_force_*` / `_absent_key_cell` /
//! `_render_available` helpers) on top of the delivered Native owners.
//!
//! # Why this is list-scoped, not a change to `Roots::load`
//!
//! The reference listing is a **display** surface: `_template_tier_roots` and
//! `list_available_detailed` read the org chain with `strict=False`
//! (`resolve_pack_chain(..., strict=False)` / `resolve_existing_org_roots`), so
//! a declared-but-unfetched org pack is *dropped from the roster* instead of
//! aborting the command. Every governance surface that needs the whole chain
//! (`context`, `activate`, `deactivate`, `preset`, `synthesize`) keeps its
//! strict read — this module is used by `list` only, and
//! [`super::Roots::load`] stays exactly as strict as it was for the others.
//!
//! An unreadable `.kittify/config.yaml` (the reference's `ValueError` family)
//! still refuses: leniency covers a *missing pack root*, never an unknown
//! registry.
//!
//! # Availability rows
//!
//! Artifact availability uses the canonical annotated availability scan, not
//! the identity index's winning-stem view. Initial root-map occurrences remain
//! distinct; later org maps add only unseen org entries, like Source's
//! `_available_over_chain`. Identity lookup supplies each row's own-layer id
//! and activation spelling, never another layer's shadowing id. Step contracts
//! use the canonical built-in-only availability layout. Mission-type rows use
//! the existing schema-validating, loud-fail `scan_mission_types_dir`.
//!
//! # Required floor
//!
//! The floor *semantics* are the canonical ones: [`read_org_required_provenance`]
//! (strict or lenient, per call site) and [`resolve_deny`] for spelling
//! matching. Nothing here re-implements the floor or deny model — the module
//! only decides which floor ids the listing shows as "in force" or "omitted",
//! exactly as the reference's `list_cmd` helpers do.
//!
//! # Read-only
//!
//! Nothing in this module writes, fetches, materialises, locks or caches. A
//! declared-but-unfetched org pack is never fetched; it simply contributes no
//! layer, and its absence is what makes an unreadable floor *reportable*
//! (`required_floor_unreadable`) rather than fatal.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use sk_charter::{
    ArtifactKind, BuiltInSearch, CHARTER_KIND_TOKENS, Limits, MISSION_TYPE_TOKEN, PackRoots,
    PathExpansion, load_project_config,
};
use sk_doctrine::{
    ActivationLedger, EffectiveWhenAbsent, IdentityIndex, IdentityLayer, ORG_MISSION_TYPES_SUBDIR,
    OfferingRoots, PROJECT_MISSION_TYPES_RELATIVE_TO_KITTIFY, RequiringPack,
    availability_entries as canonical_availability_entries, load_activation_ledger,
    org_requirable_kinds, packs_root_override, read_org_required_provenance,
    read_org_required_tokens, resolve_deny, scan_mission_types_dir,
};
use sk_kernel::get_package_asset_root;

use crate::compat::support::CompatError;

use super::list_templates;

/// The reference's empty table cell.
const EMPTY_CELL: &str = "—";
/// What an absent allow key leaves in force for a kind whose rule is "all".
const ABSENT_ALL_CELL: &str = "(All built-ins — no explicit activation)";
/// What an absent allow key leaves in force for a kind whose rule is "required".
const ABSENT_REQUIRED_CELL: &str = "(Org-required only — no explicit activation)";
/// What an explicit but empty allow list reads as.
const EMPTY_ALLOW_CELL: &str = "(Nothing activated — explicit restriction)";
/// The reference's marker for an org-required id an explicit allow list omits.
const REQUIRED_NOT_IN_FORCE: &str = "required (not in force: explicit allow list omits it)";
/// The reference's marker for an org-required id an absent allow list leaves in force.
const REQUIRED_IN_FORCE: &str = "in force: required by";
/// The reference's hint under a kind whose org required floor could not be read.
const FLOOR_UNREADABLE: &str =
    "org required floor unreadable — run `spec-kitty doctor charter-packs`";

/// The pack roots a listing reads: built-in, every **existing** configured org
/// pack (declaration order) and the project pack root when present.
///
/// Declared-but-unfetched org packs are dropped, never fetched or materialised.
#[derive(Clone, Debug)]
pub struct ListRoster {
    built_in: PathBuf,
    org_roots: Vec<PathBuf>,
    project: Option<PathBuf>,
}

impl ListRoster {
    /// Resolve the listing's roots leniently.
    ///
    /// # Errors
    ///
    /// An unreadable `.kittify/config.yaml` or an unexpandable external
    /// `local_path`, plus a missing built-in pack root (the reference's
    /// `_SRC_ROOT` is always installed, so a missing one is a real defect, not
    /// an empty layer).
    pub fn load(repo_root: &Path) -> Result<Self, CompatError> {
        let config = load_project_config(repo_root, &Limits::default())
            .map_err(|error| CompatError::domain(error.to_string()))?;
        let expansion = PathExpansion::from_env();
        // The floor/context seams accept the documented parent (`<...>/packs`) and
        // the built-in leaf spelling; the listing must resolve the same built-in
        // tier, so it reuses that one normalization instead of reading the raw env.
        let mut search = BuiltInSearch::from_env(repo_root);
        if let Some(packs_root) = packs_root_override() {
            search = search.with_packs_root_env(packs_root);
        }
        // `resolve_existing_source` is the reference's `resolve_pack_chain(...,
        // strict=False)` posture: an existing project overlay keeps its lexical
        // path, and a declared org root that is not a directory is omitted.
        let resolved =
            PackRoots::resolve_existing_source(repo_root, Some(&config), &expansion, Some(&search))
                .map_err(|error| CompatError::domain(error.to_string()))?;
        let built_in = resolved.built_in.ok_or_else(|| {
            CompatError::domain("no built-in pack root could be resolved for charter list")
        })?;
        Ok(Self {
            built_in,
            org_roots: resolved
                .organizations
                .into_iter()
                .map(|pack| pack.root)
                .collect(),
            project: resolved.project,
        })
    }

    /// The identity index over every layer the listing can read.
    fn identity(&self) -> IdentityIndex {
        IdentityIndex::from_roots(&self.built_in, &self.org_roots, self.project.as_deref())
    }

    /// The mission-type directories the listing reads, in the reference's order,
    /// annotated by layer label.
    ///
    /// The project roster lives under `.kittify/missions/mission_types` (the
    /// canonical under-`.kittify` tail constant), not under the project pack
    /// root, so it is joined from the pack root's parent.
    fn mission_type_dirs(&self, built_in_only: bool) -> Vec<(IdentityLayer, PathBuf)> {
        let mut dirs: Vec<(IdentityLayer, PathBuf)> = Vec::new();
        if let Ok(missions) = get_package_asset_root() {
            dirs.push((
                IdentityLayer::BuiltIn,
                missions.join(ORG_MISSION_TYPES_SUBDIR),
            ));
        }
        if built_in_only {
            return dirs;
        }
        for root in &self.org_roots {
            dirs.push((IdentityLayer::Org, root.join(ORG_MISSION_TYPES_SUBDIR)));
        }
        if let Some(project) = &self.project
            && let Some(kittify) = project.parent()
        {
            dirs.push((
                IdentityLayer::Project,
                kittify
                    .join(PROJECT_MISSION_TYPES_RELATIVE_TO_KITTIFY[0])
                    .join(PROJECT_MISSION_TYPES_RELATIVE_TO_KITTIFY[1]),
            ));
        }
        dirs
    }
}

/// One org-required id an explicit allow list omits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Omission {
    /// The artifact kind.
    pub kind: ArtifactKind,
    /// The activation stem the packs require.
    pub artifact_id: String,
    /// The packs that require it.
    pub packs: Vec<RequiringPack>,
}

/// One org-required id an absent allow list leaves in force.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InForce {
    /// The artifact kind.
    pub kind: ArtifactKind,
    /// The activation stem the packs require and some layer provides.
    pub stem: String,
    /// The packs that require it.
    pub packs: Vec<RequiringPack>,
}

/// The per-kind required-floor view the listing publishes.
#[derive(Clone, Debug, Default)]
pub struct Floors {
    /// Present allow lists that omit a required id (`required_not_in_force`).
    pub not_in_force: Vec<Omission>,
    /// Absent allow lists whose floor some layer provides (`required_in_force`).
    pub in_force: Vec<InForce>,
    /// Kinds whose floor could not be read strictly (`required_floor_unreadable`).
    pub unreadable: Vec<ArtifactKind>,
}

/// One artifact kind's listing row, in the reference's row order.
#[derive(Clone, Debug)]
pub struct KindRow {
    /// The operator kind token.
    pub kind: String,
    /// The authored allow list (`None` when the key is absent).
    pub activated: Option<Vec<String>>,
    /// The deny list; `None` for `mission-type` (no deny list exists).
    pub deactivated: Option<Vec<String>>,
    /// The published `available` value (`None` without `--show-available`/`--all`).
    pub available: Option<Value>,
    /// The human Activated cell (newline-separated floor markers included).
    pub activated_text: String,
    /// The human available cell.
    pub available_text: String,
}

/// A composed `charter list` view: the payload plus the text cells derived from
/// the very same rows, so the two renderings cannot drift.
#[derive(Clone, Debug)]
pub struct Listing {
    /// The `charter list --json` payload.
    pub payload: Value,
    /// Rows in canonical kind order.
    pub rows: Vec<KindRow>,
    /// The template rows when `--all` was given (`None` keeps `templates: null`).
    pub templates: Option<Value>,
}

impl Listing {
    /// Compose the listing for `repo_root`.
    ///
    /// # Errors
    ///
    /// The shared `charter list` error family: an unreadable project config or
    /// activation ledger, a built-in pack that cannot be resolved, and a
    /// malformed mission-type file (the reference's loud-fail scan).
    pub fn load(
        repo_root: &Path,
        show_available: bool,
        all_layers: bool,
    ) -> Result<Self, CompatError> {
        let roster = ListRoster::load(repo_root)?;
        let ledger = load_activation_ledger(repo_root)
            .map_err(|error| CompatError::coded(super::ACTIVATION_FAILED, error.to_string()))?;
        let identity = roster.identity();
        let floors = compose_floors(repo_root, &identity, &ledger);
        let show_available = show_available || all_layers;

        let mut rows: Vec<KindRow> = Vec::with_capacity(CHARTER_KIND_TOKENS.len());
        for token in CHARTER_KIND_TOKENS {
            let kind = artifact_kind(token);
            let activated = match kind {
                Some(kind) => ledger.allow_for(kind).map(sorted_ids),
                None => ledger
                    .raw()
                    .get(super::MISSION_TYPE_KEY)
                    .and_then(|ids| ids.as_ref())
                    .map(|ids| sorted_ids(ids)),
            };
            let deactivated = kind.map(|kind| sorted_ids(ledger.deny_for(kind)));
            let activated_set: BTreeSet<String> = activated
                .as_ref()
                .map(|ids| ids.iter().cloned().collect())
                .unwrap_or_default();

            let available = if show_available {
                Some(available_value(
                    &roster,
                    &identity,
                    kind,
                    &activated_set,
                    all_layers,
                )?)
            } else {
                None
            };
            let available_text = match &available {
                None => String::new(),
                Some(value) => available_cell(value, &identity, kind),
            };
            let activated_text =
                activated_cell(token, kind, activated.as_deref(), &floors, &identity);
            rows.push(KindRow {
                kind: token.to_owned(),
                activated,
                deactivated,
                available,
                activated_text,
                available_text,
            });
        }

        let templates = if all_layers {
            let references = list_templates::list_templates(repo_root)?;
            Some(list_templates::template_payload(&references))
        } else {
            None
        };

        let mut payload = json!({
            "result": "success",
            "kinds": rows.iter().map(row_payload).collect::<Vec<Value>>(),
            "templates": templates.clone().unwrap_or(Value::Null),
        });
        // The reference's additive payload keys: present only when non-empty.
        if let Some(object) = payload.as_object_mut() {
            if let Some(not_in_force) = required_not_in_force_json(&floors) {
                object.insert("required_not_in_force".to_owned(), not_in_force);
            }
            if let Some(in_force) = required_in_force_json(&floors) {
                object.insert("required_in_force".to_owned(), in_force);
            }
            if !floors.unreadable.is_empty() {
                object.insert(
                    "required_floor_unreadable".to_owned(),
                    Value::Array(
                        floors
                            .unreadable
                            .iter()
                            .map(|kind| Value::String(kind.plural().to_owned()))
                            .collect(),
                    ),
                );
            }
        }

        Ok(Self {
            payload,
            rows,
            templates,
        })
    }

    /// The human `template` row's available cell (`—` when nothing was found).
    #[must_use]
    pub fn template_text(&self) -> String {
        self.templates
            .as_ref()
            .map(list_templates::template_text)
            .unwrap_or_else(|| EMPTY_CELL.to_owned())
    }
}

/// The canonical artifact kind for an operator token (`None` for `mission-type`).
fn artifact_kind(token: &str) -> Option<ArtifactKind> {
    if token == MISSION_TYPE_TOKEN {
        None
    } else {
        ArtifactKind::from_operator_token(token).ok()
    }
}

/// The reference's `sorted(value)` read of a ledger list.
fn sorted_ids(ids: &[String]) -> Vec<String> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids.dedup();
    ids
}

/// One kind row's payload: exactly the reference's four keys.
fn row_payload(row: &KindRow) -> Value {
    json!({
        "kind": row.kind,
        "activated": row.activated,
        "available": row.available,
        "deactivated": row.deactivated,
    })
}

/// The floors this listing reports, composed from the canonical floor readers.
///
/// * the omission half reads leniently (`strict=false`), exactly like the
///   reference's `required_not_in_force(repo_root, strict=False)` — a listing
///   never fails because an org pack's required floor is unreadable;
/// * the in-force half reads each kind **strictly** so "cannot read" is
///   reported (`required_floor_unreadable` + the doctor hint) instead of
///   silently reading as "nothing required", exactly like
///   `_required_in_force_by_kind`.
fn compose_floors(repo_root: &Path, identity: &IdentityIndex, ledger: &ActivationLedger) -> Floors {
    let requirable = org_requirable_kinds();
    let mut floors = Floors::default();
    for token in CHARTER_KIND_TOKENS {
        let Some(kind) = artifact_kind(token) else {
            continue; // `mission-type` has no required floor and no deny list.
        };
        match ledger.allow_for(kind) {
            Some(allow) => {
                let Ok(provenance) = read_org_required_provenance(repo_root, kind, identity, false)
                else {
                    // A lenient read only fails when the registry itself cannot
                    // be read; the listing keeps its other rows rather than
                    // claiming an omission it could not establish.
                    continue;
                };
                if provenance.is_empty() {
                    continue;
                }
                let allowed = resolve_deny(kind, allow.iter().cloned(), identity).spellings();
                for stem in ordered_stems(repo_root, kind, identity, &provenance) {
                    let spelled = resolve_deny(kind, [stem.clone()], identity).spellings();
                    if spelled.is_disjoint(&allowed) {
                        floors.not_in_force.push(Omission {
                            kind,
                            artifact_id: stem.clone(),
                            packs: provenance.get(&stem).cloned().unwrap_or_default(),
                        });
                    }
                }
            }
            None => {
                if !requirable.contains(&kind) {
                    continue;
                }
                match read_org_required_provenance(repo_root, kind, identity, true) {
                    Err(_) => floors.unreadable.push(kind),
                    Ok(provenance) => {
                        let provided = provided_stems(identity, kind);
                        for stem in ordered_stems(repo_root, kind, identity, &provenance) {
                            if !provided.contains(&stem) {
                                continue;
                            }
                            floors.in_force.push(InForce {
                                kind,
                                stem: stem.clone(),
                                packs: provenance.get(&stem).cloned().unwrap_or_default(),
                            });
                        }
                    }
                }
            }
        }
    }
    floors
}

/// Floor stems in the packs' first-seen order, with the provenance map's own
/// order as the fallback for a stem the token walk cannot spell.
fn ordered_stems(
    repo_root: &Path,
    kind: ArtifactKind,
    identity: &IdentityIndex,
    provenance: &BTreeMap<String, Vec<RequiringPack>>,
) -> Vec<String> {
    let mut stems: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    if let Ok(tokens) = read_org_required_tokens(repo_root, kind, false) {
        for token in tokens {
            let stem = identity.stem(kind, &token).unwrap_or(token);
            if seen.insert(stem.clone()) {
                stems.push(stem);
            }
        }
    }
    for stem in provenance.keys() {
        if seen.insert(stem.clone()) {
            stems.push(stem.clone());
        }
    }
    stems.retain(|stem| provenance.contains_key(stem));
    stems
}

/// The stems some layer provides for `kind` (the reference's `_provided_stems`).
///
/// Read through the canonical identity index over the listing's full roster:
/// an id no layer provides is never claimed in force.
fn provided_stems(identity: &IdentityIndex, kind: ArtifactKind) -> BTreeSet<String> {
    identity.stems(kind).into_iter().collect()
}

/// The published `available` value for one kind.
///
/// `--all` yields layer-annotated rows (`{artifact_id, layer, declared_id}`) and
/// `--show-available` alone yields the reference's flat id list.
fn available_value(
    roster: &ListRoster,
    identity: &IdentityIndex,
    kind: Option<ArtifactKind>,
    activated: &BTreeSet<String>,
    all_layers: bool,
) -> Result<Value, CompatError> {
    let entries = if all_layers {
        availability_entries(roster, identity, kind, activated, true)?
    } else {
        availability_entries(roster, identity, kind, activated, false)?
    };
    if !all_layers {
        return Ok(Value::Array(
            entries
                .into_iter()
                .map(|(id, _layer)| Value::String(id))
                .collect(),
        ));
    }
    Ok(Value::Array(
        entries
            .into_iter()
            .map(|(id, layer)| {
                let declared = kind
                    .and_then(|kind| {
                        identity
                            .declared_id_in_layer(kind, &id, layer)
                            .or_else(|| identity.declared_id(kind, &id))
                    })
                    .unwrap_or_else(|| id.clone());
                json!({"artifact_id": id, "layer": layer.as_str(), "declared_id": declared})
            })
            .collect(),
    ))
}

/// The availability entries of one kind, sorted by `(id, layer)` like the
/// reference, already filtered by the activated set.
fn availability_entries(
    roster: &ListRoster,
    identity: &IdentityIndex,
    kind: Option<ArtifactKind>,
    activated: &BTreeSet<String>,
    all_layers: bool,
) -> Result<Vec<(String, IdentityLayer)>, CompatError> {
    let mut entries = Vec::new();
    match kind {
        Some(kind) => {
            let mut roots = OfferingRoots {
                built_in: Some(roster.built_in.clone()),
                org_roots: if all_layers {
                    roster.org_roots.first().cloned().into_iter().collect()
                } else {
                    Vec::new()
                },
                project: if all_layers {
                    roster.project.clone()
                } else {
                    None
                },
            };
            entries = canonical_availability_entries(kind, &roots);
            if all_layers {
                let mut seen: BTreeSet<_> = entries
                    .iter()
                    .map(|(id, layer)| (id.clone(), layer.as_str()))
                    .collect();
                roots.built_in = None;
                roots.project = None;
                for root in roster.org_roots.iter().skip(1) {
                    roots.org_roots = vec![root.clone()];
                    for (id, layer) in canonical_availability_entries(kind, &roots) {
                        if seen.insert((id.clone(), layer.as_str())) {
                            entries.push((id, layer));
                        }
                    }
                }
            }
            entries
                .retain(|(id, layer)| !is_activated(identity, kind, id, activated, Some(*layer)));
        }
        None => {
            let mut seen = BTreeSet::new();
            for (layer, directory) in roster.mission_type_dirs(!all_layers) {
                let ids = scan_mission_types_dir(&directory)
                    .map_err(|error| CompatError::domain(error.to_string()))?;
                for id in ids {
                    let Ok(id) = id.to_scalar_string() else {
                        // Missing exact-list delivery, not an inability of
                        // Kernel JSON to represent this authored Python string.
                        continue;
                    };
                    if !activated.contains(&id) && seen.insert((id.clone(), layer.as_str())) {
                        entries.push((id, layer));
                    }
                }
            }
        }
    }
    if !all_layers {
        let mut seen = BTreeSet::new();
        entries.retain(|(id, _)| seen.insert(id.clone()));
    }
    entries.sort_by(|(left, left_layer), (right, right_layer)| {
        left.cmp(right)
            .then_with(|| left_layer.as_str().cmp(right_layer.as_str()))
    });
    Ok(entries)
}

/// Whether a token is activated under either of its spellings.
///
/// Mirrors `_KindIdentity.is_activated`: a layer's own declaration is consulted
/// first so a shadowed row never borrows a higher layer's id.
fn is_activated(
    identity: &IdentityIndex,
    kind: ArtifactKind,
    token: &str,
    activated: &BTreeSet<String>,
    layer: Option<IdentityLayer>,
) -> bool {
    if activated.contains(token) {
        return true;
    }
    let declared = layer
        .and_then(|layer| identity.declared_id_in_layer(kind, token, layer))
        .or_else(|| identity.declared_id(kind, token));
    declared.is_some_and(|declared| activated.contains(&declared))
}

/// The reference's `stem (declared_id)` label, or the token when they agree.
fn label(identity: &IdentityIndex, kind: Option<ArtifactKind>, token: &str) -> String {
    let Some(kind) = kind else {
        return token.to_owned();
    };
    let Some(declared) = identity.declared_id(kind, token) else {
        return token.to_owned();
    };
    let stem = identity
        .stem(kind, token)
        .unwrap_or_else(|| token.to_owned());
    if declared == stem {
        stem
    } else {
        format!("{stem} ({declared})")
    }
}

/// The reference's Activated cell for one kind, floor markers included.
fn activated_cell(
    token: &str,
    kind: Option<ArtifactKind>,
    activated: Option<&[String]>,
    floors: &Floors,
    identity: &IdentityIndex,
) -> String {
    let mut lines: Vec<String> = vec![match activated {
        None => match kind.map(EffectiveWhenAbsent::for_kind) {
            Some(EffectiveWhenAbsent::Required) => ABSENT_REQUIRED_CELL.to_owned(),
            _ => ABSENT_ALL_CELL.to_owned(),
        },
        Some([]) => EMPTY_ALLOW_CELL.to_owned(),
        Some(ids) => ids
            .iter()
            .map(|id| label(identity, kind, id))
            .collect::<Vec<String>>()
            .join(", "),
    }];
    for omission in floors
        .not_in_force
        .iter()
        .filter(|entry| entry.kind.operator_token() == token)
    {
        lines.push(format!(
            "{} — {REQUIRED_NOT_IN_FORCE}; required by {}",
            label(identity, kind, &omission.artifact_id),
            describe_packs(&omission.packs)
        ));
    }
    for entry in floors
        .in_force
        .iter()
        .filter(|entry| entry.kind.operator_token() == token)
    {
        lines.push(format!(
            "{} — {REQUIRED_IN_FORCE} {}",
            label(identity, kind, &entry.stem),
            describe_packs(&entry.packs)
        ));
    }
    if floors
        .unreadable
        .iter()
        .any(|kind| kind.operator_token() == token)
    {
        lines.push(FLOOR_UNREADABLE.to_owned());
    }
    lines.join("\n")
}

/// The reference's `', '.join(pack.describe())`.
fn describe_packs(packs: &[RequiringPack]) -> String {
    packs
        .iter()
        .map(RequiringPack::describe)
        .collect::<Vec<String>>()
        .join(", ")
}

/// The reference's `required_not_in_force: {<plural>: [{id, packs}]}` (`None`
/// keeps the key absent when nothing is omitted).
fn required_not_in_force_json(floors: &Floors) -> Option<Value> {
    let mut by_kind: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for omission in &floors.not_in_force {
        by_kind
            .entry(omission.kind.plural())
            .or_default()
            .push(json!({
                "id": omission.artifact_id,
                "packs": pack_names(&omission.packs),
            }));
    }
    if by_kind.is_empty() {
        return None;
    }
    Some(Value::Object(
        by_kind
            .into_iter()
            .map(|(plural, rows)| (plural.to_owned(), Value::Array(rows)))
            .collect(),
    ))
}

/// The reference's `required_in_force: {<plural>: [{id, packs}]}` (`None` keeps
/// the key absent when nothing is in force).
fn required_in_force_json(floors: &Floors) -> Option<Value> {
    let mut by_kind: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for entry in &floors.in_force {
        by_kind.entry(entry.kind.plural()).or_default().push(json!({
            "id": entry.stem,
            "packs": pack_names(&entry.packs),
        }));
    }
    if by_kind.is_empty() {
        return None;
    }
    Some(Value::Object(
        by_kind
            .into_iter()
            .map(|(plural, rows)| (plural.to_owned(), Value::Array(rows)))
            .collect(),
    ))
}

/// The reference's `_pack_names`: the requiring packs' names, first-seen order.
fn pack_names(packs: &[RequiringPack]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for pack in packs {
        if !names.contains(&pack.pack) {
            names.push(pack.pack.clone());
        }
    }
    names
}

/// The human available cell, from the same rows the payload publishes.
///
/// Mirrors `_render_available` / the flat branch: an artifact is shown as
/// `<stem>` or `<stem> (<declared id>)`, and a layer-aware row adds its layer.
fn available_cell(value: &Value, identity: &IdentityIndex, kind: Option<ArtifactKind>) -> String {
    let Some(rows) = value.as_array() else {
        return EMPTY_CELL.to_owned();
    };
    if rows.is_empty() {
        return EMPTY_CELL.to_owned();
    }
    rows.iter()
        .map(|row| match row {
            Value::String(id) => label(identity, kind, id),
            other => {
                let id = other
                    .get("artifact_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let layer = other
                    .get("layer")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let declared = other
                    .get("declared_id")
                    .and_then(Value::as_str)
                    .unwrap_or(id);
                let label = if declared == id {
                    id.to_owned()
                } else {
                    format!("{id} ({declared})")
                };
                format!("{label} ({layer})")
            }
        })
        .collect::<Vec<String>>()
        .join(", ")
}
