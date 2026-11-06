//! `4.0.0rc8_promote_interview_selections`: promote answers-only interview
//! selections into PRESENT activation lists.
//!
//! Supersedes `unify_charter_activation_promote_answers`, which seeded an
//! *absent* `activated_<kind>` key with the kind's whole in-force set:
//! `charter synthesize` then read that seeded list as an operator's activation.
//! The rules here:
//!
//! * **An absent key is never written.** While `activated_<kind>` is absent every
//!   available artefact of that kind is in force, so a resolved interview id is
//!   already in force or names no available artefact.
//! * **A present list gains the interview ids it lacks**, compared as stems so a
//!   list that spells an artefact by its declared id is not given a duplicate.
//!   The write goes through the one activation write target (the activation
//!   ledger once `4.0.0rc8_activation_ledger` has moved the keys) and the one
//!   promotion rule, so the deny guard applies and a deactivated id stays
//!   deactivated.
//! * **Only where the superseded promotion never ran**: when the migration ledger
//!   records it as `success` (or as the `skipped` the shipped step itself
//!   recorded), an interview id a present list lacks is not appended back.
//! * **One run settles it**: `detect` is always true and `apply` succeeds even
//!   when nothing is left to promote, so the runner records `success`.
//!
//! * **A list an earlier build seeded is reported, never narrowed**: when a
//!   present list holds every available artefact of its kind and the interview
//!   selected only some of them, the run warns and names the narrowing commands.
//!
//! Interview ids are resolved through the layer-aware identity index
//! (`sk_doctrine::IdentityIndex`, the same authority `charter context` reads), so
//! the project pack, the org packs and the built-in pack all answer. An id no
//! layer resolves, or one no file stem can represent, is reported, never dropped
//! silently.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;
use sk_charter::synthesis::{SYNTHESIS_MANIFEST_PATH, SynthesisManifest};
use sk_charter::{ArtifactKind, compute_bundle_content_hash};
use sk_doctrine::{
    ActivationSource, IdentityIndex, NATIVE_OFFERING_SERVICE, acquire_synthesis_lock,
    apply_ledger_write, effective_deny, load_activation_ledger, prepare_ledger_write,
    resolve_activation_source, resolve_effective_sets,
};
use sk_kernel::PythonString;

use crate::error::InstallError;
use crate::ledger::{LEGACY_MIGRATION_ID_MAP, MigrationLedger, SETTLED_RESULT, SKIPPED_RESULT};
use crate::migrations::completion_legacy::paths::{
    read_text, relative_display, write_bytes, write_text,
};
use crate::upgrade::{MigrationOutcome, NativeMigration};
use crate::yaml_edit::CstDocument;

/// The interview answers record.
const ANSWERS_REL: &str = ".kittify/charter/interview/answers.yaml";
const CONFIG_REL: &str = ".kittify/config.yaml";
/// The migration this one supersedes.
const SUPERSEDED_PROMOTION_ID: &str = "unify_charter_activation_promote_answers";

const NOTHING_TO_PROMOTE: &str =
    "No interview selections to promote: every selection is already in force";
const ALREADY_PROMOTED: &str =
    "Interview selections were promoted by an earlier upgrade; nothing to promote";
const NO_SELECTIONS: &str = "No interview selections to promote";
const NOTHING_PREVIEW: &str = "dry-run: nothing would be promoted";

/// The kinds whose interview selections are promoted.
pub const PROMOTED_KINDS: [ArtifactKind; 8] = [
    ArtifactKind::Directive,
    ArtifactKind::Tactic,
    ArtifactKind::Styleguide,
    ArtifactKind::Toolguide,
    ArtifactKind::Paradigm,
    ArtifactKind::Procedure,
    ArtifactKind::AgentProfile,
    ArtifactKind::MissionStepContract,
];

/// The post-cutover id a legacy record names (upstream `_LEGACY_MIGRATION_ID_MAP`).
fn migration_id_alias(id: &str) -> &str {
    LEGACY_MIGRATION_ID_MAP
        .iter()
        .find(|(legacy, _)| *legacy == id)
        .map(|(_, current)| *current)
        .unwrap_or(id)
}

fn allow_key(kind: ArtifactKind) -> String {
    format!("activated_{}", kind.plural())
}

fn selected_ids(answers: &serde_yaml_ng::Mapping, kind: ArtifactKind) -> Vec<String> {
    let key = format!("selected_{}", kind.plural());
    match answers.get(Value::String(key)) {
        Some(Value::Sequence(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(text) => text.clone(),
                other => scalar_text(other),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(flag) => {
            if *flag {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        other => format!("{other:?}"),
    }
}

fn load_answers(project_root: &Path) -> serde_yaml_ng::Mapping {
    let path = project_root.join(ANSWERS_REL);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return serde_yaml_ng::Mapping::new();
    };
    match serde_yaml_ng::from_str::<Value>(&text) {
        Ok(Value::Mapping(mapping)) => mapping,
        _ => serde_yaml_ng::Mapping::new(),
    }
}

struct Resolution {
    stems: BTreeMap<String, Vec<String>>,
    unresolved: Vec<(String, bool)>,
}

/// Resolve every interview selection to its activation stem, through every layer.
fn resolve_selections(
    answers: &serde_yaml_ng::Mapping,
    identity: &IdentityIndex,
) -> Result<Resolution, InstallError> {
    let mut stems: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut unresolved = Vec::new();
    for kind in PROMOTED_KINDS {
        let mut chosen: Vec<String> = Vec::new();
        for raw_id in selected_ids(answers, kind) {
            if !identity.knows(kind, &raw_id) {
                unresolved.push((format!("{}:{raw_id}", kind.operator_token()), false));
                continue;
            }
            match identity.stored_spelling(kind, &raw_id) {
                Ok(stem) => {
                    if !chosen.contains(&stem) {
                        chosen.push(stem);
                    }
                }
                Err(_) => unresolved.push((format!("{}:{raw_id}", kind.operator_token()), true)),
            }
        }
        if !chosen.is_empty() {
            stems.insert(allow_key(kind), chosen);
        }
    }
    Ok(Resolution { stems, unresolved })
}

/// The activation surface a write targets.
enum Target {
    Ledger,
    Legacy(PathBuf),
}

fn activation_target(project_root: &Path) -> Result<Target, InstallError> {
    let resolved = resolve_activation_source(project_root).map_err(doctrine_error)?;
    Ok(match resolved.source() {
        ActivationSource::Ledger => Target::Ledger,
        ActivationSource::Legacy => Target::Legacy(
            resolved
                .pointer
                .clone()
                .filter(|_| resolved.pointer_exists)
                .unwrap_or_else(|| project_root.join(CONFIG_REL)),
        ),
    })
}

fn target_data(
    project_root: &Path,
    target: &Target,
) -> Result<serde_yaml_ng::Mapping, InstallError> {
    match target {
        Target::Ledger => {
            let ledger = load_activation_ledger(project_root).map_err(doctrine_error)?;
            let mut mapping = serde_yaml_ng::Mapping::new();
            for (key, value) in ledger.raw() {
                if let Some(ids) = value {
                    mapping.insert(
                        Value::String(key),
                        Value::Sequence(ids.into_iter().map(Value::String).collect()),
                    );
                }
            }
            Ok(mapping)
        }
        Target::Legacy(path) => {
            let Some(text) = read_text(path) else {
                return Ok(serde_yaml_ng::Mapping::new());
            };
            let value: Value = serde_yaml_ng::from_str(&text).map_err(|error| {
                // Source's `apply()` catches `ActiveCharterConfigError` and raises
                // `MigrationStateUnreadableError(exc.body)`: an unreadable activation
                // surface is the runner's recorded failure, never an uncaught
                // traceback and never a skip that would settle the migration.
                InstallError::MigrationStateUnreadable {
                    path: Some(path.to_path_buf()),
                    detail: format!(
                        "{} could not be read ({error}); fix the file, then run `spec-kitty upgrade` again",
                        relative_display(project_root, path)
                    ),
                }
            })?;
            Ok(match value {
                Value::Mapping(mapping) => mapping,
                _ => serde_yaml_ng::Mapping::new(),
            })
        }
    }
}

fn list_of(mapping: &serde_yaml_ng::Mapping, key: &str) -> Option<Vec<String>> {
    match mapping.get(Value::String(key.to_string())) {
        Some(Value::Sequence(items)) => Some(items.iter().map(scalar_text).collect()),
        _ => None,
    }
}

/// Whether the ledger records (or its pending adoption will record) that the
/// superseded promotion ran.
fn shipped_promotion_settled(project_root: &Path) -> Result<bool, InstallError> {
    let ledger = MigrationLedger::new(project_root);
    let records = ledger.records(false)?;
    for record in &records {
        let id = migration_id_alias(&record.id);
        if id == SUPERSEDED_PROMOTION_ID
            && (record.result == SETTLED_RESULT
                || (record.result == SKIPPED_RESULT && record.superseded_by.is_empty()))
        {
            return Ok(true);
        }
    }
    // A legacy metadata record the adoption backfill copies: the shipped step's
    // own `skipped` predates the superseded substitute.
    if let Some(metadata) = crate::metadata::ProjectMetadata::load(&project_root.join(".kittify")) {
        for record in &metadata.applied_migrations {
            if migration_id_alias(&record.id) == SUPERSEDED_PROMOTION_ID
                && record.result == SKIPPED_RESULT
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn unresolved_warning(unresolved: &[(String, bool)]) -> Option<String> {
    let mut parts = Vec::new();
    let unknown: Vec<&str> = unresolved
        .iter()
        .filter(|(_, ambiguous)| !ambiguous)
        .map(|(urn, _)| urn.as_str())
        .collect();
    let ambiguous: Vec<&str> = unresolved
        .iter()
        .filter(|(_, ambiguous)| *ambiguous)
        .map(|(urn, _)| urn.as_str())
        .collect();
    if !unknown.is_empty() {
        parts.push(format!(
            "Unresolved interview selections skipped (not promoted): {}",
            unknown.join(", ")
        ));
    }
    if !ambiguous.is_empty() {
        parts.push(format!(
            "Ambiguous interview selections skipped (not promoted): {} match more than one layer's artifact, \
             and the file name cannot stand for one of them. Rename one of the colliding files so each ID has \
             its own file name, then select the ID again.",
            ambiguous.join(", ")
        ));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn doctrine_error(error: sk_doctrine::DoctrineError) -> InstallError {
    InstallError::UpgradeRefused {
        detail: error.to_string(),
    }
}

fn restamp(project_root: &Path) -> Result<(), InstallError> {
    let path = project_root.join(SYNTHESIS_MANIFEST_PATH);
    if !path.is_file() {
        return Ok(());
    }
    let _lock = acquire_synthesis_lock(project_root, sk_doctrine::DEFAULT_LOCK_TIMEOUT)
        .map_err(doctrine_error)?;
    let manifest = match SynthesisManifest::load(&path) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Err(InstallError::UpgradeRefused {
                detail: error.to_string(),
            });
        }
    };
    if manifest.built_in_only || manifest.bundle_content_hash.is_none() {
        return Ok(());
    }
    let Some(current) = compute_bundle_content_hash(project_root) else {
        return Ok(());
    };
    if manifest.bundle_content_hash.as_deref() == Some(current.as_str()) {
        return Ok(());
    }
    let mut updated = manifest;
    updated.bundle_content_hash = Some(current);
    let finalized = updated.finalize();
    write_bytes(&path, &finalized.to_canonical_bytes()).map(|_| ())
}

/// The promotion plan of one present list.
struct Promotion {
    key: String,
    ids: Vec<String>,
    skipped_denied: Vec<String>,
}

fn plan_promotions(
    present: &BTreeMap<String, Vec<String>>,
    resolution: &Resolution,
    identity: &IdentityIndex,
    denied: &BTreeMap<String, Vec<String>>,
) -> Vec<Promotion> {
    let mut promotions = Vec::new();
    for (key, listed) in present {
        let Some(selected) = resolution.stems.get(key) else {
            continue;
        };
        let listed_stems: std::collections::BTreeSet<String> = listed
            .iter()
            .map(|token| {
                let kind = kind_for_key(key);
                match kind {
                    Some(kind) => identity
                        .stored_spelling(kind, token)
                        .unwrap_or_else(|_| token.clone()),
                    None => token.clone(),
                }
            })
            .collect();
        let mut ids = Vec::new();
        let mut skipped = Vec::new();
        for stem in selected {
            if listed_stems.contains(stem) {
                continue;
            }
            if denied.get(key).is_some_and(|ids| ids.contains(stem)) {
                skipped.push(stem.clone());
                continue;
            }
            ids.push(stem.clone());
        }
        if !ids.is_empty() || !skipped.is_empty() {
            promotions.push(Promotion {
                key: key.clone(),
                ids,
                skipped_denied: skipped,
            });
        }
    }
    promotions
}

fn kind_for_key(key: &str) -> Option<ArtifactKind> {
    let plural = key.strip_prefix("activated_")?;
    PROMOTED_KINDS
        .into_iter()
        .find(|kind| kind.plural() == plural)
}

/// One stored activation token in its activation spelling (the reference's
/// `SelectionResolver.spell`): a declared id becomes its file stem, and an
/// unknown or unrepresentable token stays exactly as written.
fn spell(identity: &IdentityIndex, kind: ArtifactKind, token: &PythonString) -> PythonString {
    identity
        .stored_spelling_exact(kind, token)
        .unwrap_or_else(|_| token.clone())
}

/// The stored spellings of `tokens` (the reference's `SelectionResolver.stems`).
fn spelled(
    identity: &IdentityIndex,
    kind: ArtifactKind,
    tokens: impl IntoIterator<Item = PythonString>,
) -> BTreeSet<PythonString> {
    tokens
        .into_iter()
        .map(|token| spell(identity, kind, &token))
        .collect()
}

/// `_seeded_list_hints`: a warning for each present list that holds every
/// available artefact of its kind while the interview selected fewer.
///
/// The in-force set is resolved once, and only for the candidate kinds (a list
/// longer than the selection). Nothing is written: narrowing is an operator
/// decision, so the result is a warning that names the narrowing commands.
fn seeded_list_hints(
    project_root: &Path,
    present: &BTreeMap<String, Vec<String>>,
    resolution: &Resolution,
    identity: &IdentityIndex,
) -> Result<Vec<String>, InstallError> {
    let mut candidates: Vec<(String, ArtifactKind)> = Vec::new();
    for kind in PROMOTED_KINDS {
        let key = allow_key(kind);
        let (Some(listed), Some(selected)) = (present.get(&key), resolution.stems.get(&key)) else {
            continue;
        };
        if listed.len() > selected.len() {
            candidates.push((key, kind));
        }
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<&str> = candidates.iter().map(|(key, _)| key.as_str()).collect();
    let sets =
        resolve_effective_sets(project_root, &keys, &NATIVE_OFFERING_SERVICE).map_err(|error| {
            InstallError::UpgradeRefused {
                detail: error.to_string(),
            }
        })?;
    let mut hints = Vec::new();
    for (key, kind) in &candidates {
        // `dict.fromkeys` keeps one entry per requested key, in request order.
        let Some(entry) = sets.iter().find(|entry| &entry.yaml_key == key) else {
            continue;
        };
        if !entry.resolved || entry.ids.is_empty() {
            continue;
        }
        let listed = spelled(
            identity,
            *kind,
            present[key]
                .iter()
                .map(|token| PythonString::from_scalars(token)),
        );
        let available = spelled(identity, *kind, entry.ids.iter().cloned());
        let selected = spelled(
            identity,
            *kind,
            resolution.stems[key]
                .iter()
                .map(|stem| PythonString::from_scalars(stem)),
        );
        if listed != available || !selected.is_subset(&listed) || selected.len() >= listed.len() {
            continue;
        }
        hints.push(format!(
            "{key} lists every available {token} ({listed}), as an upgrade before 4.0.0rc8 wrote it; \
             the interview selected {selected}. To keep only those, run `spec-kitty charter interview \
             --narrow-activation` or `spec-kitty charter activate {token} <id> --only`; \
             `charter synthesize` then asks for companion tactics only for what you keep.",
            token = kind.operator_token(),
            listed = listed.len(),
            selected = selected.len()
        ));
    }
    Ok(hints)
}

/// The effective deny (`D*`) of each promoted kind, keyed by activation key.
fn effective_denied(
    project_root: &Path,
    identity: &IdentityIndex,
    target: &Target,
) -> Result<BTreeMap<String, Vec<String>>, InstallError> {
    let mut denied = BTreeMap::new();
    if matches!(target, Target::Legacy(_)) {
        // A legacy source holds no applied deny (FR-017).
        return Ok(denied);
    }
    let ledger = load_activation_ledger(project_root).map_err(doctrine_error)?;
    let floor = sk_doctrine::required_floor(project_root, identity)
        .map(|floor| floor.tokens)
        .unwrap_or_default();
    for kind in PROMOTED_KINDS {
        let set = effective_deny(
            kind,
            ledger.deny_for(kind).iter().cloned(),
            floor.get(&kind).cloned().unwrap_or_default(),
            identity,
        );
        let mut ids: Vec<String> = set.spellings().into_iter().collect();
        ids.sort();
        denied.insert(allow_key(kind), ids);
    }
    Ok(denied)
}

/// `4.0.0rc8_promote_interview_selections.detect` — always true, so one run settles it.
pub fn detect(_project_root: &Path) -> Result<bool, InstallError> {
    Ok(true)
}

/// `4.0.0rc8_promote_interview_selections.can_apply` — always applicable.
pub fn can_apply(_project_root: &Path) -> Result<Option<String>, InstallError> {
    Ok(None)
}

/// `4.0.0rc8_promote_interview_selections.apply`.
pub fn apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    run(project_root, false)
}

/// The reference's `apply(dry_run=True)`: the promotion primitive plans against
/// a copy and writes nothing, while the run still takes the synthesis lock (the
/// reference's plan/commit seam locks before planning, which is why a dry run
/// leaves the lock file behind).
pub fn apply_dry_run(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    run(project_root, true)
}

fn run(project_root: &Path, dry_run: bool) -> Result<MigrationOutcome, InstallError> {
    let answers = load_answers(project_root);
    if PROMOTED_KINDS
        .iter()
        .all(|kind| selected_ids(&answers, *kind).is_empty())
    {
        return Ok(MigrationOutcome::unchanged(vec![NO_SELECTIONS.to_string()]));
    }
    // `SelectionResolver.build(project_path)` failing is the reference's
    // `MigrationResult(success=False, errors=["Could not resolve doctrine root: …"])`,
    // a recorded failure, never a raised error.
    let identity = match IdentityIndex::from_project(project_root) {
        Ok(identity) => identity,
        Err(error) => {
            return Ok(MigrationOutcome {
                changed: false,
                changes: Vec::new(),
                success: false,
                errors: vec![format!("Could not resolve doctrine root: {error}")],
                ..MigrationOutcome::default()
            });
        }
    };
    let resolution = resolve_selections(&answers, &identity)?;
    let target = activation_target(project_root)?;
    let data = target_data(project_root, &target)?;

    let mut present: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for kind in PROMOTED_KINDS {
        let key = allow_key(kind);
        if !resolution.stems.contains_key(&key) {
            continue;
        }
        if let Some(listed) = list_of(&data, &key) {
            present.insert(key, listed);
        }
    }

    // The reference computes the seeded-list hints BEFORE it consults the
    // superseded promotion's record, so every branch below reports them, and the
    // deny notes the primitive produces are prepended to them afterwards.
    let hints = seeded_list_hints(project_root, &present, &resolution, &identity)?;
    if shipped_promotion_settled(project_root)? {
        return Ok(MigrationOutcome {
            changed: false,
            changes: vec![if dry_run {
                NOTHING_PREVIEW.to_string()
            } else {
                ALREADY_PROMOTED.to_string()
            }],
            warnings: hints,
            ..crate::upgrade::MigrationOutcome::default()
        });
    }
    let denied = effective_denied(project_root, &identity, &target)?;
    let promotions = plan_promotions(&present, &resolution, &identity, &denied);
    let mut warnings = hints;
    if let Some(warning) = unresolved_warning(&resolution.unresolved) {
        warnings.push(warning);
    }
    if promotions.is_empty() {
        return Ok(MigrationOutcome {
            changed: false,
            changes: vec![if dry_run {
                NOTHING_PREVIEW.to_string()
            } else {
                NOTHING_TO_PROMOTE.to_string()
            }],
            warnings,
            ..crate::upgrade::MigrationOutcome::default()
        });
    }

    // The promotion primitive's own notes come first: one per key whose promotion
    // skipped an effectively deactivated id.
    let mut notes: Vec<String> = Vec::new();
    for promotion in &promotions {
        if !promotion.skipped_denied.is_empty() {
            notes.push(format!(
                "{}: skipped {}: deactivated; run `spec-kitty charter activate` to reinstate.",
                promotion.key,
                promotion
                    .skipped_denied
                    .iter()
                    .map(|id| format!("'{id}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    notes.append(&mut warnings);
    let warnings = notes;

    let _lock = acquire_synthesis_lock(project_root, sk_doctrine::DEFAULT_LOCK_TIMEOUT)
        .map_err(doctrine_error)?;
    if dry_run {
        let promoted: Vec<(String, Vec<String>)> = promotions
            .iter()
            .filter(|promotion| !promotion.ids.is_empty())
            .map(|promotion| (promotion.key.clone(), promotion.ids.clone()))
            .collect();
        let change = if promoted.is_empty() {
            NOTHING_PREVIEW.to_string()
        } else {
            format!("dry-run: would promote {}", python_dict(&promoted))
        };
        return Ok(MigrationOutcome {
            changed: !promoted.is_empty(),
            changes: vec![change],
            warnings,
            ..crate::upgrade::MigrationOutcome::default()
        });
    }

    let mut changes = Vec::new();
    let mut ledger_values: BTreeMap<String, Option<Vec<String>>> = BTreeMap::new();
    let mut legacy_edits: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for promotion in &promotions {
        if promotion.ids.is_empty() {
            continue;
        }
        let mut updated = present.get(&promotion.key).cloned().unwrap_or_default();
        updated.extend(promotion.ids.iter().cloned());
        match &target {
            Target::Ledger => {
                ledger_values.insert(promotion.key.clone(), Some(updated));
            }
            Target::Legacy(_) => {
                legacy_edits.insert(promotion.key.clone(), updated);
            }
        }
        changes.push(format!(
            "Promoted {} into {}",
            python_list(&promotion.ids),
            promotion.key
        ));
    }

    match &target {
        Target::Ledger => {
            if !ledger_values.is_empty() {
                let prepared = prepare_ledger_write(project_root, &ledger_values, &[], None)
                    .map_err(doctrine_error)?;
                apply_ledger_write(prepared).map_err(doctrine_error)?;
            }
        }
        Target::Legacy(path) => {
            if !legacy_edits.is_empty() {
                let Some(text) = read_text(path) else {
                    return Err(InstallError::UpgradeRefused {
                        detail: format!(
                            "{} could not be read; fix the file, then run `spec-kitty upgrade` again",
                            relative_display(project_root, path)
                        ),
                    });
                };
                let document = CstDocument::parse(path, &text)?;
                for (key, ids) in &legacy_edits {
                    document.root().set_string_list(key, ids);
                }
                let rendered = sk_doctrine::YamlDocument::parse(&document.text(), path)
                    .and_then(|document| document.to_source_text())
                    .map_err(doctrine_error)?;
                write_text(path, &rendered)?;
            }
        }
    }
    drop(_lock);
    if !changes.is_empty() {
        // The activation ledger is part of the bundle hash: keep the manifest
        // stamp in parity after the promotion.
        restamp(project_root)?;
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes: if changes.is_empty() {
            vec![NOTHING_TO_PROMOTE.to_string()]
        } else {
            changes
        },
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

/// A Python list literal (`['a', 'b']`), matching the Source's f-string rendering.
fn python_list(ids: &[String]) -> String {
    format!(
        "[{}]",
        ids.iter()
            .map(|id| format!("'{id}'"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// A Python dict literal (`{'a': ['x'], 'b': []}`), matching the Source's
/// f-string rendering of the promoted mapping, in promotion order.
fn python_dict(promoted: &[(String, Vec<String>)]) -> String {
    let entries: Vec<String> = promoted
        .iter()
        .map(|(key, ids)| format!("'{key}': {}", python_list(ids)))
        .collect();
    format!("{{{}}}", entries.join(", "))
}

/// The promotion migration this module implements.
pub const IMPLEMENTATIONS: &[NativeMigration] = &[NativeMigration {
    id: "4.0.0rc8_promote_interview_selections",
    detect,
    can_apply,
    apply,
}];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_python_list_literal_matches_the_source_rendering() {
        assert_eq!(
            python_list(&["a".to_string(), "b".to_string()]),
            "['a', 'b']"
        );
    }
}
