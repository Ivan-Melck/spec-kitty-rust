//! The canonical **typed** cross-pack `org-charter.yaml` fold.
//!
//! Mirrors `charter.activation.org_charter.load_org_charter_policies` — the
//! function the reference's `org_required_stems` (the `--only` promotion and
//! deny-composition writer floor) actually reads. The raw per-field reader in
//! [`super::source`] answers a *different* question for the diagnostic readers
//! and must never be substituted here: a pack whose whole document the typed
//! model refuses contributes **nothing** to this fold, while a pack it accepts
//! contributes every `required_<kind>` list it declares.
//!
//! ## What this module is not
//!
//! It is not a second schema owner: validity comes from the one typed loader
//! ([`super::load_org_charter_policy`]), which owns `OrgCharterPolicy`
//! validation. It is not a second registry/path authority: the configured packs
//! and their roots come from [`super::source::declared_org_packs`]
//! (`charter_packs.org.packs[]` + `OrgPackConfig::effective_root`). It is not a
//! second merge rule: `required_<kind>` unions through
//! [`super::union_exact_tokens`] and strips through [`super::strip_token`].
//!
//! ## Fold semantics (the reference's, in declaration order)
//!
//! * `required_<kind>` — union, preserving **first-seen** order across packs.
//! * `interview_defaults` — per-key replacement (`dict.update`): a later pack's
//!   value replaces an existing key in place and appends new keys.
//! * `governance_policies` — concatenated, deduplicated by `(field, value)`
//!   keeping the **last** occurrence at the first occurrence's position. The
//!   identity is the reference's: an authored `bool` and the text `'True'` are
//!   different values, so `(f, true)` and `(f, 'True')` are two entries, while a
//!   re-declared `(f, true)` replaces in place.
//! * `activations` — concatenated, deduplicated on the 4-tuple identity
//!   `(activation_context, charter_pack_id, artifact_id, artifact_kind)` keeping
//!   the **last** occurrence at the first occurrence's position.
//! * `org_name` — the last **truthy** value wins (a Python `str` is truthy when
//!   non-empty; it is never stripped here).
//! * `skill_namespace` — the last **non-empty after `str.strip()`** value wins.
//! * `extends` — always absent on the merged result: the fold is the resolved
//!   snapshot, not a chain link.
//! * `schema_version` — lenient: the last **truthy** value wins, with `1` as the
//!   fallback; an empty pack set yields the model default. (`_merge_chain`'s
//!   strict per-chain equality is that resolver's rule, not the cross-pack fold's.)
//!
//! ## Posture: skip vs refuse
//!
//! The reference loads each configured pack inside a `try` that re-raises its
//! operator-actionable config errors and **skips** every other failure:
//!
//! * a schema-invalid document (`ValidationError`) — the pack contributes
//!   nothing, and this fold skips it;
//! * a document the typed reader cannot even construct (the reference's YAML
//!   loader degrades to `None` on any parse failure) — skipped too;
//! * a retired field (`RetiredPackFieldError`) — **refused**, never skipped: the
//!   pack's required ids, policies and activations would vanish in silence;
//! * an org-pack config error (`OrgPackEnvVarUnsetError`,
//!   `OrgPackSubdirEscapeError`) — refused by the registry/path authority
//!   before this module sees a document.
//!
//! ## Not implemented here (disclosed, not hidden)
//!
//! * The typed pack-context door lives in this module too
//!   ([`fold_org_charter_policies_with_context`]): it builds the typed pack set
//!   from a caller [`OrgPackContext`], resolves each pack's `extends:` chain
//!   **base first** through the crate's one walk, folds every chain under strict
//!   `schema_version` equality, then cross-pack-folds the resolved packs in
//!   `pack_roots` order. That is the reference's
//!   `load_org_charter_policies(repo_root, pack_context=ctx)`; the flat entry
//!   ([`fold_org_charter_policies`]) is the reference's no-context call and stays
//!   flat (the reference ignores `extends:` on that path, so this one does too).
//! * A pack root whose directory name has no scalar spelling is left out of the
//!   typed pack set: the reference keys the set on `Path.name`, a `str` with
//!   surrogate escapes, and this crate has no lossless `OsStr` -> `PythonString`
//!   path on this door. Disclosed debt, not a silent re-spelling.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;
use sk_kernel::PythonString;

use crate::error::DoctrineError;

use super::{
    ActivationEntry, AuthoredScalar, GovernancePolicy, ORG_CHARTER_SCHEMA_VERSION,
    OrgCharterPolicy, OrgPackContext, PythonInt, chain, load_org_charter_policy,
    org_requirable_kinds, strip_token, union_exact_tokens,
};

/// One resolved snapshot of every configured org pack's `org-charter.yaml`.
///
/// Every list and string keeps exact Python code points (a requirement token may
/// be a `str` holding a lone surrogate); the authored `str | bool` policy values
/// keep their type ([`AuthoredScalar`]) and `schema_version` keeps its exact
/// Python-`int` value ([`PythonInt`]). The scalar views are explicit checked
/// adapters that drop only unrepresentable values, never invent one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrgCharterPolicyFold {
    schema_version: PythonInt,
    org_name: Option<PythonString>,
    interview_defaults: Vec<(PythonString, AuthoredScalar)>,
    required: BTreeMap<ArtifactKind, Vec<PythonString>>,
    skill_namespace: Option<PythonString>,
    governance_policies: Vec<GovernancePolicy>,
    activations: Vec<ActivationEntry>,
    packs: Vec<String>,
}

impl OrgCharterPolicyFold {
    /// The folded `schema_version` (lenient: last truthy, else `1`; the model
    /// default `2` when no pack contributed a document).
    ///
    /// It is an exact Python `int`, not a bounded projection: a Source-valid
    /// 40-digit version folds as its own value.
    pub fn schema_version(&self) -> &PythonInt {
        &self.schema_version
    }

    /// The last truthy `org_name`, exactly as authored (never stripped).
    pub fn org_name(&self) -> Option<&PythonString> {
        self.org_name.as_ref()
    }

    /// One `interview_defaults` value, exactly as the last declaring pack set it
    /// (`str | bool`, so a boolean is never the string `'True'`).
    pub fn interview_default(&self, key: &PythonString) -> Option<&AuthoredScalar> {
        self.interview_defaults
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value)
    }

    /// Every `interview_defaults` entry, in the reference's `dict` order.
    pub fn interview_defaults(&self) -> &[(PythonString, AuthoredScalar)] {
        &self.interview_defaults
    }

    /// The folded `required_<kind>` list of `kind`, exactly, first-seen order.
    pub fn required_for(&self, kind: ArtifactKind) -> &[PythonString] {
        self.required.get(&kind).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The **scalar capability view** of [`Self::required_for`].
    ///
    /// A token with no scalar projection (a lone surrogate, which is a real
    /// authored token) is omitted from this view; the exact list keeps it. The
    /// floor and identity paths never use this view.
    pub fn required_scalar(&self, kind: ArtifactKind) -> Vec<String> {
        self.required_for(kind)
            .iter()
            .filter_map(|token| token.to_scalar_string().ok())
            .collect()
    }

    /// The last non-empty-after-strip `skill_namespace`, exactly.
    pub fn skill_namespace(&self) -> Option<&PythonString> {
        self.skill_namespace.as_ref()
    }

    /// The folded `governance_policies` (deduplicated by `(field, value)`,
    /// last occurrence kept at the first occurrence's position).
    pub fn governance_policies(&self) -> &[GovernancePolicy] {
        &self.governance_policies
    }

    /// The folded `activations` (deduplicated on the reference's 4-tuple
    /// identity, last occurrence kept at the first occurrence's position).
    pub fn activations(&self) -> &[ActivationEntry] {
        &self.activations
    }

    /// The configured packs whose typed document contributed, in declaration
    /// order (a pack the typed model skipped is absent).
    pub fn contributing_packs(&self) -> &[String] {
        &self.packs
    }
}

/// Load and fold the configured org packs' `org-charter.yaml` policies.
///
/// This is `load_org_charter_policies(repo_root)` (no `pack_context`): the
/// registry's packs in declaration order, each pack's **own** document only, then
/// the cross-pack fold. `extends:` is not resolved on this path, exactly like the
/// reference — the chain resolver belongs to the pack-context door
/// ([`fold_org_charter_policies_with_context`]).
///
/// Returns an empty fold (model defaults) when no packs are configured or none
/// ship an `org-charter.yaml`.
pub fn fold_org_charter_policies(repo_root: &Path) -> Result<OrgCharterPolicyFold, DoctrineError> {
    fold_org_charter_policies_with_context(repo_root, None)
}

/// Load and fold the org packs' `org-charter.yaml` policies through the
/// reference's **pack-context** door when `context` is supplied.
///
/// * `None` — `load_org_charter_policies(repo_root)`: the registry order flat
///   fold, chains ignored (the reference's own no-context behaviour).
/// * `Some(context)` — `load_org_charter_policies(repo_root, pack_context=ctx)`:
///   the pack set is built from `context.pack_roots()` (keyed by each root's
///   directory name), every pack's `extends:` chain is resolved **base first**
///   and merged under strict `schema_version` equality, and the resolved packs
///   are then cross-pack-folded in `pack_roots` order. `repo_root` is not read on
///   this path, exactly like the reference — the context IS the pack authority.
///
/// A pack whose typed document the loader refuses is skipped (the reference's
/// `_build_pack_set` `continue`), except a retired field, which is refused. A
/// skipped pack that another pack `extends:` therefore leaves a dangling edge,
/// and the dangling edge is refused — the reference's own ordering.
pub fn fold_org_charter_policies_with_context(
    repo_root: &Path,
    context: Option<&OrgPackContext>,
) -> Result<OrgCharterPolicyFold, DoctrineError> {
    match context {
        None => fold_flat(repo_root),
        Some(context) => fold_pack_context(context),
    }
}

/// The reference's no-`pack_context` path: registry order, own documents only.
fn fold_flat(repo_root: &Path) -> Result<OrgCharterPolicyFold, DoctrineError> {
    let declared = super::source::declared_org_packs(repo_root, false)?;
    let mut policies: Vec<(String, OrgCharterPolicy)> = Vec::new();
    for (name, root) in declared {
        match load_org_charter_policy(&root) {
            Ok(Some(policy)) => policies.push((name, policy)),
            Ok(None) => {}
            // The reference's typed pack load degrades every parse/read failure
            // to `None` (its YAML load catches broadly), and its fold skips a
            // schema-invalid pack. Both are "this pack contributes nothing".
            Err(error) if fold_skips(&error) => {}
            // Retired fields and config/path errors are fail-closed.
            Err(error) => return Err(error),
        }
    }
    if policies.is_empty() {
        return Ok(OrgCharterPolicyFold::empty());
    }
    Ok(fold_policies(&policies))
}

/// The reference's `pack_context` path: `_build_pack_set` -> `_resolve_chain` ->
/// `_merge_chain` per root, then the cross-pack fold.
fn fold_pack_context(context: &OrgPackContext) -> Result<OrgCharterPolicyFold, DoctrineError> {
    // `_build_pack_set`: typed-load every root, keyed by its directory name. A
    // root without `org-charter.yaml` / with an unreadable or schema-invalid one
    // is skipped; a retired field is re-raised.
    let mut pack_set: BTreeMap<String, OrgCharterPolicy> = BTreeMap::new();
    for root in context.pack_roots() {
        let Some(name) = chain::pack_key(root) else {
            continue;
        };
        match load_org_charter_policy(root) {
            Ok(Some(policy)) => {
                pack_set.insert(name, policy);
            }
            Ok(None) => {}
            Err(error) if fold_skips(&error) => {}
            Err(error) => return Err(error),
        }
    }
    if pack_set.is_empty() {
        // The reference returns the model default (`OrgCharterPolicy()`) when the
        // pack set is empty — never an error.
        return Ok(OrgCharterPolicyFold::empty());
    }

    let edges: BTreeMap<String, Option<PythonString>> = pack_set
        .iter()
        .map(|(name, policy)| {
            (
                name.clone(),
                policy.extends.as_deref().map(PythonString::from_scalars),
            )
        })
        .collect();

    // Resolve every root in `pack_roots` order. A root absent from the pack set
    // (no usable document) contributes nothing, exactly like the reference.
    let mut resolved: Vec<(String, OrgCharterPolicy)> = Vec::new();
    for root in context.pack_roots() {
        let Some(name) = chain::pack_key(root) else {
            continue;
        };
        let Some(policy) = pack_set.get(&name) else {
            continue;
        };
        let order = chain::chain_base_first(&name, &edges, &policy.path)?;
        let chain_policies: Vec<OrgCharterPolicy> = order
            .iter()
            .filter_map(|member| pack_set.get(member).cloned())
            .collect();
        resolved.push((name, merge_chain(&chain_policies)?));
    }
    if resolved.is_empty() {
        return Ok(OrgCharterPolicyFold::empty());
    }
    // Cross-pack fold: lenient `schema_version` (last truthy wins); strict
    // equality was already enforced per chain inside `merge_chain`.
    Ok(fold_policies(&resolved))
}

/// Whether the reference's fold **skips** (rather than re-raises) this error.
fn fold_skips(error: &DoctrineError) -> bool {
    matches!(
        error,
        DoctrineError::InvalidOrgCharter { .. } | DoctrineError::OrgCharterReadAborted { .. }
    )
}

impl OrgCharterPolicyFold {
    /// The fold of no policies: the reference's empty `OrgCharterPolicy()`.
    fn empty() -> Self {
        Self {
            schema_version: PythonInt::from_i64(ORG_CHARTER_SCHEMA_VERSION),
            org_name: None,
            interview_defaults: Vec::new(),
            required: org_requirable_kinds()
                .into_iter()
                .map(|kind| (kind, Vec::new()))
                .collect(),
            skill_namespace: None,
            governance_policies: Vec::new(),
            activations: Vec::new(),
            packs: Vec::new(),
        }
    }
}

/// The fold body: the reference's `_fold_policies(policies, strict_schema_version=False)`.
fn fold_policies(policies: &[(String, OrgCharterPolicy)]) -> OrgCharterPolicyFold {
    // Lenient `schema_version`: last truthy value wins, `1` fallback (see NOTE in
    // the reference: `0` is falsy, so it never becomes the folded version).
    let mut accumulator = FoldAccumulator::new(PythonInt::from_i64(1));
    let mut packs = Vec::with_capacity(policies.len());
    for (name, policy) in policies {
        packs.push(name.clone());
        if policy.schema_version.is_truthy() {
            accumulator.schema_version = policy.schema_version.clone();
        }
        accumulator.push(policy);
    }
    accumulator.into_fold(packs)
}

/// Merge one base-first `extends:` chain: the reference's `_merge_chain`.
///
/// Thin use of the SAME accumulation body as [`fold_policies`], with the strict
/// `schema_version` rule the reference applies per chain: every pack in the chain
/// must share one version, except that versions `1` and `2` may mix (#3732 — `2`
/// only adds the retired-field rejection), and the folded version is the maximum.
/// A mismatch is a refusal, never a silent pick.
fn merge_chain(chain_policies: &[OrgCharterPolicy]) -> Result<OrgCharterPolicy, DoctrineError> {
    let (path, pack_root) = match chain_policies.last() {
        Some(policy) => (policy.path.clone(), policy.pack_root.clone()),
        None => (PathBuf::new(), PathBuf::new()),
    };
    let mut accumulator = FoldAccumulator::new(chain_schema_version(chain_policies, &path)?);
    for policy in chain_policies {
        accumulator.push(policy);
    }
    Ok(accumulator.into_policy(path, pack_root))
}

/// The reference's strict per-chain `schema_version` rule (`_resolve_fold_schema_version`).
fn chain_schema_version(
    chain_policies: &[OrgCharterPolicy],
    path: &Path,
) -> Result<PythonInt, DoctrineError> {
    let mut versions: Vec<&PythonInt> = Vec::new();
    for policy in chain_policies {
        if !versions.iter().any(|seen| **seen == policy.schema_version) {
            versions.push(&policy.schema_version);
        }
    }
    // `versions <= {1, 2}`: mixing the two compatible generations is allowed.
    let compatible = versions
        .iter()
        .all(|version| matches!(version.as_decimal(), "1" | "2"));
    if versions.len() > 1 && !compatible {
        versions.sort();
        let sorted: Vec<&str> = versions.iter().map(|v| v.as_decimal()).collect();
        return Err(DoctrineError::InvalidOrgCharter {
            path: path.to_path_buf(),
            message: format!(
                "schema_version mismatch in extends: chain. Versions found: [{}]. \
                 All packs in a chain must share the same schema_version.",
                sorted.join(", ")
            ),
        });
    }
    Ok(versions
        .into_iter()
        .max()
        .cloned()
        .unwrap_or_else(|| PythonInt::from_i64(ORG_CHARTER_SCHEMA_VERSION)))
}

/// The reference's `_fold_policies` loop body, shared by the flat/cross-pack fold
/// and the per-chain merge so the two cannot disagree about merge semantics.
struct FoldAccumulator {
    schema_version: PythonInt,
    org_name: Option<PythonString>,
    interview_defaults: Vec<(PythonString, AuthoredScalar)>,
    required: BTreeMap<ArtifactKind, Vec<PythonString>>,
    skill_namespace: Option<PythonString>,
    governance: Vec<GovernancePolicy>,
    activations: Vec<ActivationEntry>,
}

impl FoldAccumulator {
    fn new(schema_version: PythonInt) -> Self {
        Self {
            schema_version,
            org_name: None,
            interview_defaults: Vec::new(),
            required: org_requirable_kinds()
                .into_iter()
                .map(|kind| (kind, Vec::new()))
                .collect(),
            skill_namespace: None,
            governance: Vec::new(),
            activations: Vec::new(),
        }
    }

    /// Fold one policy in (declaration order): last truthy `org_name`, last
    /// non-empty stripped `skill_namespace`, per-key `interview_defaults`
    /// replacement (first slot, last value), first-seen `required_<kind>` union.
    fn push(&mut self, policy: &OrgCharterPolicy) {
        // `if policy.org_name:` — Python truthiness of a `str | None`.
        if let Some(candidate) = &policy.org_name
            && !candidate.codepoints().is_empty()
        {
            self.org_name = Some(candidate.clone());
        }
        // `last_non_empty_token`: strip the candidate, keep the last non-empty.
        if let Some(candidate) = &policy.skill_namespace {
            let stripped = strip_token(candidate);
            if !stripped.is_empty() {
                self.skill_namespace = Some(stripped);
            }
        }
        // `dict.update`: replace in place, append new keys.
        for (key, value) in &policy.interview_defaults {
            match self
                .interview_defaults
                .iter_mut()
                .find(|(candidate, _)| candidate == key)
            {
                Some(slot) => slot.1 = value.clone(),
                None => self.interview_defaults.push((key.clone(), value.clone())),
            }
        }
        for kind in org_requirable_kinds() {
            if let Some(into) = self.required.get_mut(&kind) {
                union_exact_tokens(into, policy.required_for(kind).iter().cloned());
            }
        }
        self.governance
            .extend(policy.governance_policies.iter().cloned());
        self.activations.extend(policy.activations.iter().cloned());
    }

    fn into_fold(self, packs: Vec<String>) -> OrgCharterPolicyFold {
        OrgCharterPolicyFold {
            schema_version: self.schema_version,
            org_name: self.org_name,
            interview_defaults: self.interview_defaults,
            required: self.required,
            skill_namespace: self.skill_namespace,
            governance_policies: dedupe_governance(self.governance),
            activations: dedupe_activations(self.activations),
            packs,
        }
    }

    /// The resolved chain as the model it stands for: `extends` absent (a
    /// resolved snapshot is not a chain link), carrying the overlay's path.
    fn into_policy(self, path: PathBuf, pack_root: PathBuf) -> OrgCharterPolicy {
        OrgCharterPolicy {
            path,
            pack_root,
            schema_version: self.schema_version,
            extends: None,
            org_name: self.org_name,
            interview_defaults: self.interview_defaults.into_iter().collect(),
            required: self.required,
            skill_namespace: self.skill_namespace,
            governance_policies: dedupe_governance(self.governance),
            activations: dedupe_activations(self.activations),
        }
    }
}

/// Deduplicate by `(field, value)` keeping the last entry at the first position.
///
/// `value` is the authored [`AuthoredScalar`], so the reference's Python identity
/// is preserved exactly: `Bool(true) != Text("True")`, and a repeated identical
/// value (same type, same code points) replaces in place.
fn dedupe_governance(entries: Vec<GovernancePolicy>) -> Vec<GovernancePolicy> {
    let mut deduped: Vec<GovernancePolicy> = Vec::with_capacity(entries.len());
    for entry in entries {
        match deduped
            .iter()
            .position(|existing| existing.field == entry.field && existing.value == entry.value)
        {
            Some(position) => deduped[position] = entry,
            None => deduped.push(entry),
        }
    }
    deduped
}

/// Deduplicate on the reference's 4-tuple identity, last entry at the first position.
fn dedupe_activations(entries: Vec<ActivationEntry>) -> Vec<ActivationEntry> {
    let mut deduped: Vec<ActivationEntry> = Vec::with_capacity(entries.len());
    for entry in entries {
        match deduped
            .iter()
            .position(|existing| existing.same_identity(&entry))
        {
            Some(position) => deduped[position] = entry,
            None => deduped.push(entry),
        }
    }
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> AuthoredScalar {
        AuthoredScalar::Text(PythonString::from_scalars(value))
    }

    fn policy(
        root: &str,
        schema_version: PythonInt,
        org_name: Option<&str>,
        interview_defaults: &[(&str, AuthoredScalar)],
        required: &[&str],
        skill_namespace: Option<&str>,
        governance: &[(&str, AuthoredScalar, &str)],
    ) -> OrgCharterPolicy {
        OrgCharterPolicy {
            path: std::path::PathBuf::from(format!("{root}/org-charter.yaml")),
            pack_root: std::path::PathBuf::from(root),
            schema_version,
            extends: Some("ghost".to_owned()),
            org_name: org_name.map(PythonString::from_scalars),
            interview_defaults: interview_defaults
                .iter()
                .map(|(key, value)| (PythonString::from_scalars(key), value.clone()))
                .collect(),
            required: BTreeMap::from([(
                ArtifactKind::Directive,
                required
                    .iter()
                    .map(|token| PythonString::from_scalars(token))
                    .collect(),
            )]),
            skill_namespace: skill_namespace.map(PythonString::from_scalars),
            governance_policies: governance
                .iter()
                .map(|(field, value, enforcement)| GovernancePolicy {
                    field: PythonString::from_scalars(field),
                    value: value.clone(),
                    enforcement: PythonString::from_scalars(enforcement),
                })
                .collect(),
            activations: Vec::new(),
        }
    }

    #[test]
    fn empty_fold_is_the_model_default_with_every_requirable_kind_present() {
        let fold = OrgCharterPolicyFold::empty();
        assert_eq!(
            fold.schema_version().as_decimal(),
            ORG_CHARTER_SCHEMA_VERSION.to_string()
        );
        assert!(fold.org_name().is_none());
        assert!(fold.skill_namespace().is_none());
        assert!(fold.governance_policies().is_empty());
        assert!(fold.activations().is_empty());
        assert!(fold.contributing_packs().is_empty());
        for kind in org_requirable_kinds() {
            assert!(fold.required_for(kind).is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn fold_semantics_are_first_seen_union_and_last_wins_metadata() {
        let base = policy(
            "/base",
            PythonInt::from_i64(1),
            Some("Base"),
            &[("a", text("one")), ("b", text("keep"))],
            &["D1", "D2"],
            Some("  base  "),
            &[("f", text("first"), "advisory")],
        );
        let overlay = policy(
            "/overlay",
            PythonInt::from_i64(0),
            // An empty `org_name` is falsy and never replaces a truthy one.
            Some(""),
            &[("a", text("two"))],
            &["D2", "D3"],
            Some("   "),
            &[("f", text("first"), "strict")],
        );
        let fold = fold_policies(&[("base".to_owned(), base), ("overlay".to_owned(), overlay)]);
        assert_eq!(
            fold.required_scalar(ArtifactKind::Directive),
            ["D1", "D2", "D3"],
            "first-seen union order"
        );
        assert_eq!(
            fold.org_name().map(|name| name.to_scalar_string().unwrap()),
            Some("Base".to_owned()),
            "an empty later org_name never replaces a truthy one"
        );
        assert_eq!(
            fold.skill_namespace()
                .map(|namespace| namespace.to_scalar_string().unwrap()),
            Some("base".to_owned()),
            "a blank later namespace is not a last non-empty token"
        );
        assert_eq!(
            fold.interview_default(&PythonString::from_scalars("a")),
            Some(&text("two"))
        );
        assert_eq!(
            fold.interview_default(&PythonString::from_scalars("b")),
            Some(&text("keep"))
        );
        // Last occurrence wins, first position kept.
        assert_eq!(fold.governance_policies().len(), 1);
        assert_eq!(
            fold.governance_policies()[0]
                .enforcement
                .to_scalar_string()
                .unwrap(),
            "strict"
        );
        assert_eq!(fold.contributing_packs(), ["base", "overlay"]);
        // `schema_version: 0` is falsy, so the earlier truthy version survives.
        assert_eq!(fold.schema_version().as_decimal(), "1");
    }

    #[test]
    fn authored_bool_and_text_are_distinct_governance_identities() {
        // Probed against the pinned Source: `(f, true)` and `(f, 'True')` are two
        // entries (a bool is not the string `'True'`), a repeated identical
        // `(f, true)` replaces in place, and the fold's order is first-position /
        // last-value.
        let base = policy(
            "/base",
            PythonInt::from_i64(2),
            None,
            &[("flag", AuthoredScalar::Bool(true)), ("text", text("x"))],
            &[],
            None,
            &[
                ("f", AuthoredScalar::Bool(true), "advisory"),
                ("g", text("True"), "advisory"),
            ],
        );
        let overlay = policy(
            "/overlay",
            PythonInt::from_i64(2),
            None,
            &[("flag", text("True"))],
            &[],
            None,
            &[
                ("f", text("True"), "strict"),
                ("g", AuthoredScalar::Bool(true), "strict"),
                ("f", AuthoredScalar::Bool(true), "advisory"),
            ],
        );
        let fold = fold_policies(&[("base".to_owned(), base), ("overlay".to_owned(), overlay)]);
        let entries: Vec<(String, AuthoredScalar, String)> = fold
            .governance_policies()
            .iter()
            .map(|entry| {
                (
                    entry.field.to_scalar_string().unwrap(),
                    entry.value.clone(),
                    entry.enforcement.to_scalar_string().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            entries,
            vec![
                (
                    "f".to_owned(),
                    AuthoredScalar::Bool(true),
                    "advisory".to_owned()
                ),
                ("g".to_owned(), text("True"), "advisory".to_owned()),
                ("f".to_owned(), text("True"), "strict".to_owned()),
                (
                    "g".to_owned(),
                    AuthoredScalar::Bool(true),
                    "strict".to_owned()
                ),
            ],
            "bool vs text is a distinct identity; the later identical `(f, true)` replaced in place"
        );
        // Per-key replacement keeps the authored type of the last declaring pack.
        assert_eq!(
            fold.interview_default(&PythonString::from_scalars("flag")),
            Some(&text("True"))
        );
        assert_eq!(
            fold.interview_default(&PythonString::from_scalars("text")),
            Some(&text("x"))
        );
    }

    #[test]
    fn wide_and_unicode_schema_versions_fold_exactly() {
        let wide = PythonInt::parse_text(
            &PythonString::from_scalars("1234567890123456789012345678901234567890"),
            4300,
        )
        .expect("a 40-digit Python int");
        let base = policy("/base", PythonInt::from_i64(0), None, &[], &[], None, &[]);
        let overlay = policy("/overlay", wide.clone(), None, &[], &[], None, &[]);
        let fold = fold_policies(&[("base".to_owned(), base), ("overlay".to_owned(), overlay)]);
        assert_eq!(fold.schema_version(), &wide);
        assert_eq!(
            fold.schema_version().as_decimal(),
            "1234567890123456789012345678901234567890"
        );
    }
}
