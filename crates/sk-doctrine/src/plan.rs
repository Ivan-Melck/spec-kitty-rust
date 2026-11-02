//! Pure plan / single-write commit seam for charter activation.
//!
//! Mirrors `src/charter/activation/activation_engine.py`. Planning validates
//! the artifact id **before** computing any post-state and never writes;
//! committing applies one plan through one locked, lossless write.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;

use crate::error::DoctrineError;
use crate::ledger::{
    ActivationSource, DenyListChoice, DenySet, EffectiveWhenAbsent, RequiringPack,
    check_deactivation_permitted, deny_key, prepare_ledger_write, refuse_unmigrated_deny_write,
    resolve_activation_source,
};
use crate::yaml::{PreparedWrite, YamlDocument};

/// What is effective for one activation key while that key is absent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveSet {
    /// The artifact kind operator token.
    pub kind: String,
    /// The activation key.
    pub yaml_key: String,
    /// The effective ids, as activation stems.
    pub ids: Vec<String>,
    /// Best-effort readable-chain union; never written by promotion.
    pub fallback_ids: Vec<String>,
    /// Whether the offering could be resolved.
    pub resolved: bool,
    /// Why the set is unresolved, when it is.
    pub reason: Option<String>,
}

/// The in-memory post-state of one activation or deactivation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ActivationPlan {
    /// The resolved activation key.
    pub yaml_key: String,
    /// The post-state allow list; `None` leaves the allow state as it is.
    pub new_list: Option<Vec<String>>,
    /// Operator-facing warnings.
    pub warnings: Vec<String>,
    /// Cascade targets keyed by kind.
    pub cascade_targets: BTreeMap<String, Vec<String>>,
    /// IDs newly added by this plan.
    pub activated: Vec<String>,
    /// IDs removed by this plan.
    pub deactivated: Vec<String>,
    /// The deny key this plan changes.
    pub deny_key: Option<String>,
    /// The post-state deny list (`[]` removes the key).
    pub new_deny_list: Option<Vec<String>>,
    /// The explicit choice authorising the deny change.
    pub deny_change: Option<DenyListChoice>,
    /// Whether the plan changes nothing.
    pub noop: bool,
}

/// Appended to warnings that materialise an absent key into an explicit list.
pub const FROZEN_LIST_NOTE: &str = " This list is now explicit: artifacts shipped later are not activated automatically; use 'spec-kitty charter activate <kind> <id> --only' to keep activation narrow.";

/// Why an activation that changes nothing writes nothing.
pub const ALREADY_IN_FORCE_WARNING: &str = "{kind} {artifact} is already in force; nothing written. A list would freeze {kind}. Use `spec-kitty charter activate {kind} {artifact} --only` to write one.";

/// `(added, cleared)` between two deny-lists, sorted; `None` reads as empty.
// Public compatibility seam (the coordinator CLI may diff two plans); used by
// tests today.
#[allow(dead_code)]
pub fn deny_delta(
    before: Option<&[String]>,
    after: Option<&[String]>,
) -> (Vec<String>, Vec<String>) {
    let old: BTreeSet<&String> = before.into_iter().flatten().collect();
    let new: BTreeSet<&String> = after.into_iter().flatten().collect();
    (
        new.difference(&old).map(|id| (*id).clone()).collect(),
        old.difference(&new).map(|id| (*id).clone()).collect(),
    )
}

/// Whether a kind's absent activation key keeps every artifact in force.
///
/// The one rule behind every writer that must not fill an absent key: an
/// absent `activated_<plural>` of a kind whose absence means "all" would only
/// freeze the catalog (or narrow it) if written.
pub fn absent_key_stays_implicit(
    effective_when_absent: EffectiveWhenAbsent,
    key_present: bool,
) -> bool {
    !key_present && effective_when_absent == EffectiveWhenAbsent::All
}

/// The warning a narrowing caller emits when what stopped being in force cannot be listed.
// Public compatibility seam for the `--only` caller.
#[allow(dead_code)]
pub fn narrow_unresolved_warning(reason: Option<&str>) -> String {
    const BASE: &str =
        "What stopped being in force cannot be listed: the effective set could not be resolved.";
    match reason {
        None => BASE.to_owned(),
        Some(reason) => format!(
            "What stopped being in force cannot be listed: the effective set could not be resolved because {reason}."
        ),
    }
}

/// The one narrow post-state: `base` plus `selection`; what stops being in force is reported.
pub fn plan_narrowed_list(
    yaml_key: &str,
    selection: &[String],
    base: &[String],
    previous: &[String],
) -> ActivationPlan {
    let chosen = dedupe(selection);
    let in_force = dedupe(previous);
    let mut new_list = dedupe(base);
    for id in &chosen {
        if !new_list.contains(id) {
            new_list.push(id.clone());
        }
    }
    ActivationPlan {
        yaml_key: yaml_key.to_owned(),
        new_list: Some(new_list.clone()),
        activated: chosen
            .iter()
            .filter(|id| !in_force.contains(id))
            .cloned()
            .collect(),
        deactivated: in_force
            .iter()
            .filter(|id| !new_list.contains(id))
            .cloned()
            .collect(),
        ..ActivationPlan::default()
    }
}

fn freeze_warnings(
    kind: &str,
    yaml_key: &str,
    effective_when_absent: EffectiveWhenAbsent,
) -> Vec<String> {
    if effective_when_absent != EffectiveWhenAbsent::Required {
        return Vec::new();
    }
    let plural = yaml_key.strip_prefix("activated_").unwrap_or(yaml_key);
    vec![format!(
        "Kind '{kind}' now has an explicit activation list, so the org packs' required {plural} are frozen at their current values: a {kind} an org pack later adds to required_{plural} will not reach this project."
    )]
}

fn current_list(
    document: &YamlDocument,
    yaml_key: &str,
) -> Result<Option<Vec<String>>, DoctrineError> {
    document.get_string_list(yaml_key)
}

fn deny_list(deny_key: Option<&str>, current_deny: &[String]) -> Vec<String> {
    if deny_key.is_none() {
        Vec::new()
    } else {
        current_deny.to_vec()
    }
}

fn with_deny(
    mut plan: ActivationPlan,
    deny_key: Option<&str>,
    deny: &[String],
    cleared: &str,
    choice: DenyListChoice,
) -> ActivationPlan {
    let Some(deny_key) = deny_key else {
        return plan;
    };
    if !deny.iter().any(|item| item == cleared) {
        return plan;
    }
    plan.deny_key = Some(deny_key.to_owned());
    plan.new_deny_list = Some(
        deny.iter()
            .filter(|item| *item != cleared)
            .cloned()
            .collect(),
    );
    plan.deny_change = Some(choice);
    plan
}

fn materialise_absent(
    kind: &str,
    effective_ids: &[String],
    available_ids: &[String],
    effective_when_absent: EffectiveWhenAbsent,
    in_force: bool,
    exclude: &DenySet,
) -> (Vec<String>, Vec<String>) {
    let mut materialized: Vec<String> = Vec::new();
    for id in effective_ids {
        if exclude.admits(id) && !materialized.contains(id) {
            materialized.push(id.clone());
        }
    }
    if materialized.is_empty() && effective_when_absent == EffectiveWhenAbsent::All {
        for id in available_ids {
            if exclude.admits(id) && !materialized.contains(id) {
                materialized.push(id.clone());
            }
        }
    }
    let mut warnings = Vec::new();
    if in_force {
        let note = if effective_when_absent == EffectiveWhenAbsent::Required {
            ""
        } else {
            FROZEN_LIST_NOTE
        };
        warnings.push(format!(
            "Kind '{kind}' had no explicit activation set. Initialized from the {} artifact(s) already effective, so nothing in force was deactivated.{note}",
            materialized.len()
        ));
    }
    (materialized, warnings)
}

fn stays_implicit(
    current: Option<&[String]>,
    artifact_id: &str,
    effective_ids: &[String],
    effective_when_absent: EffectiveWhenAbsent,
    effective_ids_in_force: bool,
) -> bool {
    if !absent_key_stays_implicit(effective_when_absent, current.is_some())
        || !effective_ids_in_force
    {
        return false;
    }
    effective_ids.iter().any(|id| id == artifact_id)
}

/// The inputs to [`plan_activation`], passed as data (no discovery, no I/O).
pub struct ActivationInputs<'a> {
    /// The `config.yaml` key for the kind.
    pub yaml_key: &'a str,
    /// The universe of valid artifact ids for the kind.
    pub available_ids: &'a [String],
    /// The already-loaded config mapping.
    pub config_data: &'a YamlDocument,
    /// What the activation-aware resolver currently has in force.
    pub effective_ids: &'a [String],
    /// What an absent key puts in force.
    pub effective_when_absent: EffectiveWhenAbsent,
    /// Whether `effective_ids` is what was in force.
    pub effective_ids_in_force: bool,
    /// Opt-in narrow activation (`--only`): the base list.
    pub narrow_base: Option<&'a [String]>,
    /// What was in force before a narrowing.
    pub previous_ids: &'a [String],
    /// The kind's deny key, when it has one.
    pub deny_key: Option<&'a str>,
    /// The stored deny list (never read from the write-target mapping).
    pub current_deny: &'a [String],
    /// The explicit choice recorded for a cleared exclusion.
    pub deny_change: DenyListChoice,
    /// `D*` in stem spelling, left out of an absent-list materialisation.
    pub effective_deny: &'a DenySet,
}

/// Compute the post-state for activating `artifact_id` — purely.
///
/// The artifact id is validated before any post-state is computed. On an
/// unknown id no plan exists, so no commit (and no write) is possible.
pub fn plan_activation(
    kind: &str,
    artifact_id: &str,
    inputs: &ActivationInputs<'_>,
) -> Result<ActivationPlan, DoctrineError> {
    if !inputs.available_ids.iter().any(|id| id == artifact_id) {
        return Err(DoctrineError::UnknownActivationId {
            kind: kind.to_owned(),
            artifact_id: artifact_id.to_owned(),
        });
    }
    let current = current_list(inputs.config_data, inputs.yaml_key)?;
    let deny = deny_list(inputs.deny_key, inputs.current_deny);

    if let Some(base) = inputs.narrow_base {
        let mut plan = plan_narrowed_list(
            inputs.yaml_key,
            &[artifact_id.to_owned()],
            base,
            inputs.previous_ids,
        );
        plan.warnings = freeze_warnings(kind, inputs.yaml_key, inputs.effective_when_absent);
        return Ok(with_deny(
            plan,
            inputs.deny_key,
            &deny,
            artifact_id,
            inputs.deny_change,
        ));
    }

    if current.is_none()
        && inputs.effective_when_absent == EffectiveWhenAbsent::All
        && deny.iter().any(|id| id == artifact_id)
    {
        let plan = ActivationPlan {
            yaml_key: inputs.yaml_key.to_owned(),
            new_list: None,
            activated: vec![artifact_id.to_owned()],
            warnings: vec![format!(
                "Kind '{kind}' keeps no explicit activation set, so artifacts shipped later stay active."
            )],
            ..ActivationPlan::default()
        };
        return Ok(with_deny(
            plan,
            inputs.deny_key,
            &deny,
            artifact_id,
            inputs.deny_change,
        ));
    }

    if stays_implicit(
        current.as_deref(),
        artifact_id,
        inputs.effective_ids,
        inputs.effective_when_absent,
        inputs.effective_ids_in_force,
    ) {
        let plan = ActivationPlan {
            yaml_key: inputs.yaml_key.to_owned(),
            new_list: None,
            activated: vec![artifact_id.to_owned()],
            warnings: vec![
                ALREADY_IN_FORCE_WARNING
                    .replace("{kind}", kind)
                    .replace("{artifact}", artifact_id),
            ],
            ..ActivationPlan::default()
        };
        return Ok(with_deny(
            plan,
            inputs.deny_key,
            &deny,
            artifact_id,
            inputs.deny_change,
        ));
    }

    let mut warnings = Vec::new();
    let mut new_list = match &current {
        Some(current) => current.clone(),
        None => {
            let mut exclude = inputs.effective_deny.clone();
            // The activated id is being cleared, not excluded, from the materialised set.
            exclude.stems.remove(artifact_id);
            exclude.declared_ids.remove(artifact_id);
            exclude.urns.remove(&format!("{kind}:{artifact_id}"));
            let (materialized, mut init_warnings) = materialise_absent(
                kind,
                inputs.effective_ids,
                inputs.available_ids,
                inputs.effective_when_absent,
                inputs.effective_ids_in_force,
                &exclude,
            );
            warnings.append(&mut init_warnings);
            warnings.extend(freeze_warnings(
                kind,
                inputs.yaml_key,
                inputs.effective_when_absent,
            ));
            materialized
        }
    };

    let mut activated = Vec::new();
    if new_list.iter().any(|id| id == artifact_id) && current.is_some() {
        warnings.push(format!(
            "'{artifact_id}' is already activated for kind '{kind}'."
        ));
    } else {
        if !new_list.iter().any(|id| id == artifact_id) {
            new_list.push(artifact_id.to_owned());
        }
        activated.push(artifact_id.to_owned());
    }

    let plan = ActivationPlan {
        yaml_key: inputs.yaml_key.to_owned(),
        new_list: Some(new_list),
        warnings,
        activated,
        ..ActivationPlan::default()
    };
    Ok(with_deny(
        plan,
        inputs.deny_key,
        &deny,
        artifact_id,
        inputs.deny_change,
    ))
}

fn dedupe(ids: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    ids.iter()
        .filter(|id| seen.insert((*id).clone()))
        .cloned()
        .collect()
}

/// The inputs to [`plan_deactivation`].
pub struct DeactivationInputs<'a> {
    /// The `config.yaml` key for the kind.
    pub yaml_key: &'a str,
    /// The already-loaded config mapping.
    pub config_data: &'a YamlDocument,
    /// The universe of valid artifact ids for the kind.
    pub available_ids: &'a [String],
    /// The required floor, keyed by artifact id.
    pub required_by: &'a BTreeMap<String, Vec<RequiringPack>>,
    /// The kind's deny key.
    pub deny_key: Option<&'a str>,
    /// The stored deny list.
    pub current_deny: &'a [String],
    /// Whether to record an exclusion (a cascade child does not).
    pub record_exclusion: bool,
    /// Additional spellings that name the same artifact.
    pub also_matches: &'a [String],
}

/// Compute the post-state for deactivating `artifact_id` — purely.
///
/// Order: the required-floor refusal, then the unknown-id refusal, then the
/// plan. An id already denied is a flagged no-op.
pub fn plan_deactivation(
    kind: &str,
    artifact_id: &str,
    inputs: &DeactivationInputs<'_>,
) -> Result<ActivationPlan, DoctrineError> {
    check_deactivation_permitted(kind, artifact_id, inputs.required_by)?;
    let current = current_list(inputs.config_data, inputs.yaml_key)?;
    let deny = deny_list(inputs.deny_key, inputs.current_deny);
    let mut spellings: BTreeSet<&str> = BTreeSet::from([artifact_id]);
    spellings.extend(inputs.also_matches.iter().map(String::as_str));
    let known = inputs.available_ids.iter().any(|id| id == artifact_id)
        || current
            .iter()
            .flatten()
            .any(|id| spellings.contains(id.as_str()))
        || deny.iter().any(|id| spellings.contains(id.as_str()));
    if !known {
        return Err(DoctrineError::UnknownActivationId {
            kind: kind.to_owned(),
            artifact_id: artifact_id.to_owned(),
        });
    }

    let records = inputs.record_exclusion && inputs.deny_key.is_some();
    if records && deny.iter().any(|id| id == artifact_id) {
        return Ok(ActivationPlan {
            yaml_key: inputs.yaml_key.to_owned(),
            new_list: current,
            noop: true,
            warnings: vec![format!(
                "'{artifact_id}' is already deactivated for kind '{kind}'. Nothing was recorded."
            )],
            ..ActivationPlan::default()
        });
    }

    let removed: Vec<String> = current
        .iter()
        .flatten()
        .filter(|id| spellings.contains(id.as_str()))
        .cloned()
        .collect();
    let new_list = current.as_ref().map(|current| {
        current
            .iter()
            .filter(|id| !spellings.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>()
    });
    if removed.is_empty() && !records {
        return Ok(ActivationPlan {
            yaml_key: inputs.yaml_key.to_owned(),
            new_list: current,
            noop: true,
            warnings: vec![format!(
                "'{artifact_id}' is not in the activation set for kind '{kind}'. Nothing to deactivate."
            )],
            ..ActivationPlan::default()
        });
    }

    let mut plan = ActivationPlan {
        yaml_key: inputs.yaml_key.to_owned(),
        new_list,
        deactivated: vec![artifact_id.to_owned()],
        ..ActivationPlan::default()
    };
    if !records {
        return Ok(plan);
    }
    let mut post: BTreeSet<String> = deny.iter().cloned().collect();
    post.insert(artifact_id.to_owned());
    plan.deny_key = inputs.deny_key.map(str::to_owned);
    plan.new_deny_list = Some(post.into_iter().collect());
    plan.deny_change = Some(DenyListChoice::Deactivate);
    Ok(plan)
}

/// Apply a plan to a lossless document, in memory. Nothing is written.
pub fn apply_plan(document: &mut YamlDocument, plan: &ActivationPlan) {
    if let Some(new_list) = &plan.new_list {
        document.set_string_list(&plan.yaml_key, new_list);
    }
    if let Some(deny_key) = &plan.deny_key {
        match &plan.new_deny_list {
            Some(ids) if !ids.is_empty() => {
                let mut ids = ids.clone();
                ids.sort();
                ids.dedup();
                document.set_string_list(deny_key, &ids);
            }
            _ => document.remove(deny_key),
        }
    }
}

/// Commit a plan through the one locked, lossless, single write.
///
/// A deny change against an unmigrated legacy surface is refused **before**
/// preparing bytes. A plan that changes nothing prepares an unchanged write.
pub fn commit_plan(
    repo_root: &Path,
    plan: &ActivationPlan,
) -> Result<PreparedWrite, DoctrineError> {
    let resolved = resolve_activation_source(repo_root)?;
    let target_is_ledger = resolved.source() == ActivationSource::Ledger || resolved.ledger_exists;
    if !target_is_ledger {
        if plan.deny_key.is_some() || plan.deny_change.is_some() {
            refuse_unmigrated_deny_write(repo_root)?;
            return Err(DoctrineError::UnmigratedDenyWrite {
                keys: plan.deny_key.clone().unwrap_or_default(),
                files: repo_root.join(".kittify/config.yaml").display().to_string(),
            });
        }
        let (target, document) = legacy_target(repo_root, &resolved)?;
        let mut document = document;
        apply_plan(&mut document, plan);
        let desired = document.to_text().into_bytes();
        return PreparedWrite::prepare(&target, desired);
    }

    let mut values: BTreeMap<String, Option<Vec<String>>> = BTreeMap::new();
    if let Some(new_list) = &plan.new_list {
        values.insert(plan.yaml_key.clone(), Some(new_list.clone()));
    }
    if let Some(deny_key) = &plan.deny_key {
        let ids = plan.new_deny_list.clone().unwrap_or_default();
        values.insert(
            deny_key.clone(),
            if ids.is_empty() { None } else { Some(ids) },
        );
    }
    prepare_ledger_write(repo_root, &values, &[], plan.deny_change)
}

fn legacy_target(
    repo_root: &Path,
    resolved: &crate::ledger::ResolvedActivationSource,
) -> Result<(PathBuf, YamlDocument), DoctrineError> {
    let config_path = repo_root.join(crate::ledger::CONFIG_YAML_REL);
    if let Some(pointer) = &resolved.pointer {
        if !resolved.pointer_exists {
            return Err(DoctrineError::Yaml {
                path: config_path,
                message: format!(
                    ".kittify/config.yaml 'charter:' pointer names {}, which does not exist",
                    pointer.display()
                ),
            });
        }
        return Ok((pointer.clone(), YamlDocument::open(pointer)?));
    }
    let document = YamlDocument::open(&config_path)?;
    Ok((config_path, document))
}

/// The activation key for an artifact kind (`activated_<plural>`).
pub fn activation_key(kind: ArtifactKind) -> Result<String, DoctrineError> {
    crate::ledger::allow_key(kind)
}

/// The deny key for an artifact kind, when it has one.
pub fn kind_deny_key(kind: ArtifactKind) -> Option<String> {
    deny_key(kind).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::IdentityIndex;
    use std::fs;

    fn document(text: &str) -> YamlDocument {
        YamlDocument::parse(text, Path::new("<memory>")).unwrap()
    }

    static EMPTY_DENY: std::sync::LazyLock<DenySet> = std::sync::LazyLock::new(DenySet::empty);

    fn inputs<'a>(
        yaml_key: &'a str,
        available: &'a [String],
        config: &'a YamlDocument,
    ) -> ActivationInputs<'a> {
        ActivationInputs {
            yaml_key,
            available_ids: available,
            config_data: config,
            effective_ids: &[],
            effective_when_absent: EffectiveWhenAbsent::All,
            effective_ids_in_force: true,
            narrow_base: None,
            previous_ids: &[],
            deny_key: Some("deactivated_directives"),
            current_deny: &[],
            deny_change: DenyListChoice::Activate,
            effective_deny: &EMPTY_DENY,
        }
    }

    #[test]
    fn unknown_id_is_refused_before_any_post_state() {
        let config = document("activated_directives: [a]\n");
        let available = vec!["a".to_owned()];
        let error = plan_activation(
            "directive",
            "missing",
            &inputs("activated_directives", &available, &config),
        )
        .unwrap_err();
        assert!(matches!(error, DoctrineError::UnknownActivationId { .. }));
    }

    #[test]
    fn absent_key_materialises_what_is_in_force_and_never_narrows() {
        let config = document("{}\n");
        let available = vec!["a".to_owned(), "b".to_owned()];
        let effective = vec!["a".to_owned()];
        let mut options = inputs("activated_directives", &available, &config);
        options.effective_ids = &effective;
        let plan = plan_activation("directive", "b", &options).unwrap();
        assert_eq!(plan.new_list, Some(vec!["a".to_owned(), "b".to_owned()]));
        assert!(
            plan.warnings
                .iter()
                .any(|warning| warning.contains("nothing in force was deactivated"))
        );
        assert!(
            plan.warnings
                .iter()
                .any(|warning| warning.contains("shipped later are not activated automatically"))
        );
    }

    #[test]
    fn already_in_force_activation_writes_no_list() {
        let config = document("{}\n");
        let available = vec!["a".to_owned()];
        let effective = vec!["a".to_owned()];
        let mut options = inputs("activated_directives", &available, &config);
        options.effective_ids = &effective;
        let plan = plan_activation("directive", "a", &options).unwrap();
        assert_eq!(plan.new_list, None);
        assert!(plan.warnings[0].contains("already in force"));
    }

    #[test]
    fn narrow_activation_reports_what_stopped_being_in_force() {
        let config = document("activated_directives: [a, b, c]\n");
        let available = vec![
            "a".to_owned(),
            "b".to_owned(),
            "c".to_owned(),
            "d".to_owned(),
        ];
        let base = vec!["a".to_owned()];
        let previous = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let mut options = inputs("activated_directives", &available, &config);
        options.narrow_base = Some(&base);
        options.previous_ids = &previous;
        let plan = plan_activation("directive", "d", &options).unwrap();
        assert_eq!(plan.new_list, Some(vec!["a".to_owned(), "d".to_owned()]));
        assert_eq!(plan.deactivated, vec!["b".to_owned(), "c".to_owned()]);
    }

    #[test]
    fn denied_id_activation_clears_the_exclusion() {
        let config = document("{}\n");
        let available = vec!["a".to_owned()];
        let deny = vec!["a".to_owned()];
        let mut options = inputs("activated_directives", &available, &config);
        options.current_deny = &deny;
        let plan = plan_activation("directive", "a", &options).unwrap();
        assert_eq!(plan.new_list, None);
        assert_eq!(plan.new_deny_list, Some(Vec::new()));
        assert_eq!(plan.deny_change, Some(DenyListChoice::Activate));
    }

    #[test]
    fn deactivation_refuses_required_and_records_exclusion() {
        let config = document("activated_directives: [a, b]\n");
        let available = vec!["a".to_owned(), "b".to_owned()];
        let required = BTreeMap::from([(
            "a".to_owned(),
            vec![RequiringPack {
                pack: "acme".to_owned(),
                source: "org-charter.yaml".to_owned(),
                policy_key: "required_directives".to_owned(),
            }],
        )]);
        let base = DeactivationInputs {
            yaml_key: "activated_directives",
            config_data: &config,
            available_ids: &available,
            required_by: &required,
            deny_key: Some("deactivated_directives"),
            current_deny: &[],
            record_exclusion: true,
            also_matches: &[],
        };
        assert!(matches!(
            plan_deactivation("directive", "a", &base),
            Err(DoctrineError::RequiredArtifact { .. })
        ));
        let plan = plan_deactivation("directive", "b", &base).unwrap();
        assert_eq!(plan.new_list, Some(vec!["a".to_owned()]));
        assert_eq!(plan.new_deny_list, Some(vec!["b".to_owned()]));
        assert_eq!(plan.deny_change, Some(DenyListChoice::Deactivate));

        let already_denied = DeactivationInputs {
            current_deny: &["b".to_owned()],
            ..base
        };
        let plan = plan_deactivation("directive", "b", &already_denied).unwrap();
        assert!(plan.noop);
    }

    #[test]
    fn commit_refuses_unmigrated_deny_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join(".kittify")).unwrap();
        fs::write(
            root.join(".kittify/config.yaml"),
            "activated_directives: [a]\n",
        )
        .unwrap();
        let plan = ActivationPlan {
            yaml_key: "activated_directives".to_owned(),
            new_list: Some(vec!["a".to_owned()]),
            deny_key: Some("deactivated_directives".to_owned()),
            new_deny_list: Some(vec!["b".to_owned()]),
            deny_change: Some(DenyListChoice::Deactivate),
            ..ActivationPlan::default()
        };
        assert!(commit_plan(root, &plan).is_err());
        assert_eq!(
            fs::read_to_string(root.join(".kittify/config.yaml")).unwrap(),
            "activated_directives: [a]\n"
        );
        assert!(!root.join(".kittify/charter/activation.yaml").exists());
    }

    #[test]
    fn commit_writes_a_ledger_losslessly_and_verifies() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join(".kittify/charter")).unwrap();
        let ledger = root.join(".kittify/charter/activation.yaml");
        fs::write(&ledger, "# ledger\nactivated_directives: []\n").unwrap();
        let plan = ActivationPlan {
            yaml_key: "activated_directives".to_owned(),
            new_list: Some(vec!["a".to_owned()]),
            ..ActivationPlan::default()
        };
        let prepared = commit_plan(root, &plan).unwrap();
        prepared.apply(crate::yaml::DEFAULT_LOCK_TIMEOUT).unwrap();
        let text = fs::read_to_string(&ledger).unwrap();
        assert!(text.contains("# ledger"));
        assert!(text.contains("activated_directives: [a]"));
        let _index = IdentityIndex::from_roots(root, &[], None);
    }
}
