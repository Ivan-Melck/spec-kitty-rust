//! The activation gate the canonical offering service applies (`PackContext` + `_gated`).
//!
//! The reference's raw offering service (`CharterOfferingService`) applies **no**
//! activation filter; the activation-aware wrapper (`ActiveCharterService`) wraps
//! every gated mapping through `_gated`
//! (`src/charter/activation/resolver.py`). This module is the ONE Native gate
//! composition:
//!
//! * the canonical exact activation ledger is read **once**;
//! * Source `load_denied` is called with that supplied ledger and the supplied
//!   org roots, so its deny identity is built fresh at the reader's own phase
//!   (`crate::source_denied`);
//! * the effective deny is the canonical `deny_by_kind` / `ExactDenySet`
//!   algebra, never a scalar projection;
//! * the Source pending-kind strict floor and the absent/null
//!   `activated_skills` org-required default are preserved;
//! * allow tokens resolve through the canonical fallible exact identity queries
//!   and are cached per `(kind, DenySet)` with Source's first-resolution
//!   publication timing.
//!
//! An allow token that names a delivered item but that the identity index does
//! not know is kept verbatim (the reference's service-only bypass). A denied
//! repository key is excluded even when the allow set names it. An identity scan
//! abort is a real `ConstructorAbort`, never a healthy unresolved token.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;
use sk_kernel::PythonString;
use sk_kernel::python_logging::{LoggingPolicy, WarningDelivery, deliver_warning_with};
use sk_kernel::python_stream::StreamError;
use sk_kernel::python_yaml::string_set_order;

use crate::identity::IdentityIndex;
use crate::ledger::{
    ActivationReadError, ExactDenySet, activatable_kinds, load_activation_ledger_exact,
};
use crate::offering::{ConstructorAbort, NativeOfferingFailure};
use crate::org_charter::read_org_required_ids_exact;
use crate::source_denied::{LoadDeniedOptions, deny_by_kind, load_denied};

/// One present allow set: the reference's netted `frozenset` in **iteration order**.
///
/// Source builds the set twice when a deny nets it — `_read_activated_<kind>` materialises
/// `frozenset(<authored list>)` and `_net` materialises `frozenset(token for token in
/// ids if deny.admits(token))` — and `_gated` then iterates the result, so the
/// warning order is the second set's table order, not the authored order and not a
/// sorted one. [`string_set_order`] is that table authority.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct AllowSet {
    order: Vec<PythonString>,
}

impl AllowSet {
    /// Materialise the reference's netted set from the authored token sequence.
    fn build(
        tokens: Vec<PythonString>,
        deny: Option<&ExactDenySet>,
    ) -> Result<Self, sk_kernel::python_yaml::HashSeedError> {
        let authored = string_set_order(tokens)?;
        let order = match deny {
            // Source `_net` returns `ids` unchanged when the kind has no deny entry.
            None => authored,
            Some(deny) => string_set_order(
                authored
                    .into_iter()
                    .filter(|token| deny.admits_exact(token))
                    .collect(),
            )?,
        };
        Ok(Self { order })
    }
}

/// The delivery outcomes of the most recent activation resolution, in order.
///
/// The reference's `_gated` calls `_LOGGER.warning` once per collected message and
/// then publishes its cache; this records what each of those calls did so a caller
/// can see a suppressed or handler-handled delivery instead of inferring it.
///
/// A delivery that cannot happen at all (a start-up policy error or a Source-valid
/// stream backend this native composition does not carry) is retained here as
/// [`Self::escaped`] **and** returned as a typed failure: it never publishes the
/// resolution cache and never returns healthy membership.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WarningLog {
    outcomes: Vec<WarningDelivery>,
    escaped: Option<StreamError>,
}

impl WarningLog {
    /// One outcome per delivered attempt, in collection order.
    #[must_use]
    pub fn outcomes(&self) -> &[WarningDelivery] {
        &self.outcomes
    }

    /// The stream failure that stopped delivery, when there was one.
    ///
    /// This is evidence of a typed Native failure (an unusable start-up stream
    /// configuration or a Source-valid backend this composition lacks), not a
    /// healthy answer: the caller also receives `Err` and no resolved ids are
    /// cached, so a retry reattempts delivery instead of hitting a cache.
    #[must_use]
    pub fn escaped(&self) -> Option<&StreamError> {
        self.escaped.as_ref()
    }

    /// Whether no delivery was attempted (the reference had no warnings).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.outcomes.is_empty() && self.escaped.is_none()
    }
}

/// Why one activation-gate filtering/resolution attempt did not produce membership.
///
/// The two arms are deliberately distinct: [`Self::Constructor`] is the reference's
/// **actual** runtime exception (`_gated`'s identity scan), while [`Self::Native`] is
/// a Native output/backend capability failure that the reference cannot reach and
/// that is never Source-invalid data or an ordinary unresolved result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GateFailure {
    /// The real Source runtime exception the loader does not catch.
    Constructor(ConstructorAbort),
    /// A typed Native capability/start-up failure.
    Native(NativeOfferingFailure),
}

impl From<ConstructorAbort> for GateFailure {
    fn from(abort: ConstructorAbort) -> Self {
        Self::Constructor(abort)
    }
}

impl From<NativeOfferingFailure> for GateFailure {
    fn from(native: NativeOfferingFailure) -> Self {
        Self::Native(native)
    }
}

impl GateFailure {
    /// The real Source exception, when this failure is one.
    #[must_use]
    pub fn abort(&self) -> Option<&ConstructorAbort> {
        match self {
            Self::Constructor(abort) => Some(abort),
            Self::Native(_) => None,
        }
    }

    /// The typed Native cause, when this failure is one.
    #[must_use]
    pub fn native(&self) -> Option<&NativeOfferingFailure> {
        match self {
            Self::Constructor(_) => None,
            Self::Native(native) => Some(native),
        }
    }
}

impl std::fmt::Display for GateFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Constructor(abort) => std::fmt::Display::fmt(abort, formatter),
            Self::Native(native) => std::fmt::Display::fmt(native, formatter),
        }
    }
}

impl std::error::Error for GateFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Constructor(abort) => Some(abort),
            Self::Native(native) => Some(native),
        }
    }
}

/// The effective per-kind allow/deny state one service build reads.
///
/// `allow` keeps the reference's three-state shape: `None` is "key absent", a
/// present set is deny-netted. The resolved allow ids are a per-gate cache keyed
/// by the exact `(kind, DenySet)` pair; a failed resolution publishes nothing.
#[derive(Clone, Debug, Default)]
pub struct ActivationGate {
    /// `None` is the reference's "key absent"; a present set is deny-netted.
    allow: BTreeMap<ArtifactKind, Option<AllowSet>>,
    /// The effective deny `D*` per kind (empty sets are omitted).
    deny: BTreeMap<ArtifactKind, ExactDenySet>,
    /// The identity index allow tokens resolve through.
    identity: IdentityIndex,
    /// `_gated`'s `_activated_id_cache`, published only after a successful loop.
    resolved: RefCell<BTreeMap<(ArtifactKind, ExactDenySet), BTreeSet<PythonString>>>,
    /// The process logging state the warning delivery consults.
    logging: LoggingPolicy,
    /// What the most recent resolution's `_LOGGER.warning` calls did.
    delivery: RefCell<WarningLog>,
}

impl PartialEq for ActivationGate {
    /// Two gates are equal when they decide the same way; the derived identity
    /// index and the resolution cache are not part of the decision surface.
    fn eq(&self, other: &Self) -> bool {
        self.allow == other.allow && self.deny == other.deny
    }
}

impl Eq for ActivationGate {}

impl ActivationGate {
    /// An unfiltered gate (`pack_context=None` in the reference): every key is
    /// admitted, for callers that deliberately bypass activation.
    #[must_use]
    pub fn unfiltered() -> Self {
        Self::default()
    }

    /// Build the gate for one repository with typed activation-read failures.
    ///
    /// This is the primary construction seam. It composes `PackContext.from_config`
    /// and its one `load_denied` call: the exact ledger is read once, the deny
    /// reader receives that ledger and these org roots, and only pending denied
    /// kinds read a strict required floor.
    ///
    /// # Errors
    ///
    /// The exact ledger/config read, the supplied-ledger deny read (including a
    /// pending kind's strict floor and a fresh identity scan abort), the Source
    /// `required_skills` default read, or a plural-taxonomy mismatch.
    pub fn try_from_project(
        repo_root: &Path,
        built_in_root: &Path,
        org_roots: &[PathBuf],
        project_root: Option<&Path>,
    ) -> Result<Self, ActivationReadError> {
        // Allow-token resolution is the service identity. It is deliberately not
        // passed into `load_denied`: the reader owns its own fresh deny index.
        let identity = IdentityIndex::from_roots(built_in_root, org_roots, project_root);
        let ledger = load_activation_ledger_exact(repo_root)?;
        let denied = load_denied(
            repo_root,
            &LoadDeniedOptions {
                ledger: Some(&ledger),
                org_roots: Some(org_roots),
                kinds: None,
            },
        )?;
        let deny = deny_by_kind(&denied)?;

        let mut allow = BTreeMap::new();
        for kind in activatable_kinds() {
            // Source `_read_activated_skills` has its own absent/null default:
            // `required_skills` through the lenient, coercing org reader. Every
            // other kind's absent key stays `None` (unrestricted), and a present
            // `[]` stays present-empty.
            let declared = match ledger.allow_for(kind) {
                Some(tokens) => Some(tokens.to_vec()),
                None if kind == ArtifactKind::Skill => {
                    Some(read_org_required_ids_exact(repo_root, kind)?)
                }
                None => None,
            };
            // `PackContext.from_config` nets a present allow list through `D*`,
            // which materialises the reference's second `frozenset`.
            let present = match declared {
                None => None,
                Some(tokens) => {
                    Some(AllowSet::build(tokens, deny.get(&kind)).map_err(|cause| {
                        // CPython refuses to start with an unusable `PYTHONHASHSEED`, so no
                        // reference code path constructs a set at all. This is a typed Native
                        // start-up-policy cause, never a Source exception class.
                        ActivationReadError::Native(NativeOfferingFailure::ActivationSetOrder {
                            phase: "activation set order".to_owned(),
                            path: repo_root.to_path_buf(),
                            cause,
                        })
                    })?)
                }
            };
            allow.insert(kind, present);
        }
        Ok(Self {
            allow,
            deny,
            identity,
            resolved: RefCell::new(BTreeMap::new()),
            logging: LoggingPolicy::source_default(),
            delivery: RefCell::new(WarningLog::default()),
        })
    }

    /// Checked compatibility facade over [`Self::try_from_project`].
    ///
    /// The service-build renderer in the reference is a string, so the existing
    /// callers keep that shape. This is not the cause authority: the typed
    /// [`ActivationReadError`] stays available through `try_from_project`, and
    /// filter identity aborts propagate through [`crate::offering::OfferingLoadError`].
    ///
    /// # Errors
    ///
    /// The activation-ledger/config read, deny/floor read, `required_skills`
    /// default, or taxonomy failure, rendered as the calling service's reason.
    pub fn from_project(
        repo_root: &Path,
        built_in_root: &Path,
        org_roots: &[PathBuf],
        project_root: Option<&Path>,
    ) -> Result<Self, String> {
        Self::try_from_project(repo_root, built_in_root, org_roots, project_root)
            .map_err(|error| error.to_string())
    }

    /// Start one service's lazy identity and resolution caches afresh.
    ///
    /// `PackContext` data may be reused, but Source creates `_identity` and
    /// `_activated_id_cache` on each `ActiveCharterService`, not on that context.
    /// Rebinding the same roots performs no read and keeps allow/deny snapshots.
    pub(crate) fn reset_service_runtime(&mut self) {
        self.identity = IdentityIndex::from_roots(
            &self.identity.built_in_root,
            &self.identity.org_roots,
            self.identity.project_root.as_deref(),
        );
        self.resolved.get_mut().clear();
        self.delivery.get_mut().clone_from(&WarningLog::default());
    }

    /// Whether the repository key `key` survives the exact deny set.
    fn deny_admits(&self, kind: ArtifactKind, key: &PythonString) -> bool {
        self.deny
            .get(&kind)
            .is_none_or(|deny| deny.admits_exact(key))
    }

    /// Deliver warnings under an explicit process logging policy.
    ///
    /// The reference reads `logging.disable`, `logging.lastResort.level` and
    /// `logging.raiseExceptions` from process globals that a program configures at
    /// start-up. Native has no logging configuration surface, so this seam lets a
    /// caller reproduce an already-configured process; it is not a new knob on the
    /// gate's own decision surface and it does not change what `filter` returns.
    #[must_use]
    pub fn with_logging_policy(mut self, policy: LoggingPolicy) -> Self {
        self.logging = policy;
        self
    }

    /// What the most recent activation resolution's warning deliveries did.
    #[must_use]
    pub fn warning_log(&self) -> WarningLog {
        self.delivery.borrow().clone()
    }

    /// Whether `kind` has a configured allow set (the reference's three-state test).
    #[must_use]
    pub fn allow_configured(&self, kind: ArtifactKind) -> bool {
        matches!(self.allow.get(&kind), Some(Some(_)))
    }
    /// The resolved activation ids for one kind, from the ONE per-gate cache.
    ///
    /// The token loop runs in the reference's set-iteration order, the collected
    /// messages are delivered through the default `_LOGGER.warning` path, and the
    /// cache is published only after that: exactly `_gated`'s order. A delivery that
    /// cannot happen returns a typed Native failure with the delivery evidence and
    /// publishes nothing; an identity scan abort is a real [`ConstructorAbort`] and
    /// also delivers/publishes nothing.
    fn resolved_ids(
        &self,
        kind: ArtifactKind,
        allow: &AllowSet,
        items: &BTreeMap<PythonString, ()>,
    ) -> Result<BTreeSet<PythonString>, GateFailure> {
        let key = (kind, self.deny.get(&kind).cloned().unwrap_or_default());
        if let Some(cached) = self.resolved.borrow().get(&key) {
            return Ok(cached.clone());
        }
        let mut resolved = BTreeSet::new();
        let mut warnings: Vec<PythonString> = Vec::new();
        for token in &allow.order {
            // The reference's service-only bypass: a token that names a
            // delivered item but is absent from the identity index is kept as
            // written (without a warning).
            if items.contains_key(token) && !self.identity.try_knows_exact(kind, token)? {
                resolved.insert(token.clone());
            } else {
                let (id, warning) = self.identity.try_activation_report_exact(kind, token)?;
                if let Some(warning) = warning {
                    warnings.push(warning);
                }
                resolved.insert(id);
            }
        }
        let mut log = WarningLog::default();
        for warning in &warnings {
            match deliver_warning_with(self.logging, warning) {
                Ok(outcome) => log.outcomes.push(outcome),
                Err(escaped) => {
                    log.escaped = Some(escaped.clone());
                    *self.delivery.borrow_mut() = log.clone();
                    // The delivery evidence is retained, the typed cause travels, and
                    // nothing is published: a retry reattempts instead of reading a
                    // healthy cache entry.
                    return Err(GateFailure::Native(
                        NativeOfferingFailure::ActivationWarningDelivery {
                            cause: Box::new(escaped),
                            log: Box::new(log),
                        },
                    ));
                }
            }
        }
        *self.delivery.borrow_mut() = log;
        // Source publishes `_activated_id_cache[kind, deny]` right after the
        // `_LOGGER.warning` loop, so a suppression or a handler-handled write
        // still publishes.
        self.resolved.borrow_mut().insert(key, resolved.clone());
        Ok(resolved)
    }

    /// The keys of `items` the gate keeps, in the map's key order.
    ///
    /// `items` is the raw repository mapping (repository key → item); the gate
    /// only decides membership, never rewrites a key.
    ///
    /// # Errors
    ///
    /// The typed gate failure: a real [`ConstructorAbort`] from the identity scan
    /// (the reference's `_gated` phase) or a typed Native delivery/capability
    /// failure — never a healthy unresolved or admitted key.
    pub fn filter<T>(
        &self,
        kind: ArtifactKind,
        items: &BTreeMap<PythonString, T>,
    ) -> Result<Vec<PythonString>, GateFailure> {
        match self.allow.get(&kind) {
            None | Some(None) => Ok(items
                .keys()
                .filter(|key| self.deny_admits(kind, key))
                .cloned()
                .collect()),
            Some(Some(allow)) => {
                let keys: BTreeMap<PythonString, ()> =
                    items.keys().map(|key| (key.clone(), ())).collect();
                let resolved = self.resolved_ids(kind, allow, &keys)?;
                Ok(items
                    .keys()
                    .filter(|key| resolved.contains(*key) && self.deny_admits(kind, key))
                    .cloned()
                    .collect())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> PythonString {
        PythonString::from_scalars(text)
    }

    fn items(names: &[&str]) -> BTreeMap<PythonString, ()> {
        names.iter().map(|name| (key(name), ())).collect()
    }

    #[test]
    fn three_state_allow_and_deny_semantics() {
        let unfiltered = ActivationGate::unfiltered();
        let mut gate = ActivationGate::unfiltered();
        gate.allow.insert(
            ArtifactKind::Tactic,
            Some(AllowSet {
                order: vec![key("b")],
            }),
        );
        let mut deny = ExactDenySet::empty();
        deny.declared_ids.insert(key("b"));
        gate.deny.insert(ArtifactKind::Tactic, deny);
        let all = items(&["a", "b", "c"]);
        assert_eq!(
            unfiltered.filter(ArtifactKind::Tactic, &all).unwrap(),
            vec![key("a"), key("b"), key("c")]
        );
        assert!(
            gate.filter(ArtifactKind::Tactic, &all).unwrap().is_empty(),
            "a deny wins over the allow set"
        );
        gate.allow
            .insert(ArtifactKind::Tactic, Some(AllowSet { order: Vec::new() }));
        assert!(
            gate.filter(ArtifactKind::Tactic, &all).unwrap().is_empty(),
            "an empty allow set keeps nothing"
        );
        gate.allow.insert(ArtifactKind::Tactic, None);
        assert_eq!(
            gate.filter(ArtifactKind::Tactic, &all).unwrap(),
            vec![key("a"), key("c")]
        );
    }

    #[test]
    fn a_failed_first_resolution_publishes_no_cache_entry() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("builtin")).unwrap();
        std::fs::create_dir_all(root.join("org/tactics")).unwrap();
        std::fs::create_dir_all(root.join(".kittify/charter")).unwrap();
        std::fs::write(
            root.join(".kittify/config.yaml"),
            "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: org\n",
        )
        .unwrap();
        std::fs::write(root.join("org/org-charter.yaml"), "{}\n").unwrap();
        std::fs::write(
            root.join(".kittify/charter/activation.yaml"),
            "activated_tactics: [alpha]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("org/tactics/bad.tactic.yaml"),
            "id: !!timestamp 2021-13-45\n",
        )
        .unwrap();
        let gate = ActivationGate::try_from_project(
            root,
            &root.join("builtin"),
            &[root.join("org")],
            None,
        )
        .expect("no pending deny, so construction reads no identity");
        let all = BTreeMap::from([(key("alpha"), ())]);
        assert!(gate.filter(ArtifactKind::Tactic, &all).is_err());
        assert!(
            gate.resolved.borrow().is_empty(),
            "a failed first resolution publishes no ids"
        );
    }
}
