//! The canonical effective-set seam (`charter.activation.effective_set`).
//!
//! Answers one question: *what is effective for an activation key while that key
//! is absent?* The reference asks it in exactly one place
//! (`src/charter/activation/effective_set.py::resolve_effective_sets`) and every
//! absent-key reasoner consumes that answer. Reading a shipped id list instead is
//! a strict subset of the offering and would narrow the set on promotion, so this
//! module reproduces the reference's whole-offering answer:
//!
//! * the built-in layer, the project pack root and **every** declared org pack
//!   (the reference's `layer_roots["org"]` is pack #1 only, a back-compat
//!   contract; the reference scans once per declared root and so does this);
//! * for every kind except `directive`, the keys of the activation-aware
//!   doctrine service's own mapping, mapped to their activation stem through
//!   [`IdentityIndex`] (a key the index does not know is kept, so a
//!   service-only artifact cannot vanish).
//!
//! Every id is emitted as an **activation stem**, so a writer that seeds an
//! absent key never stores both spellings.
//!
//! # The service half is real
//!
//! [`OfferingContext`] carries the repository context the reference's
//! `build_active_charter_service(repo_root)` derives from the repository itself:
//! the resolved pack roots **and** the inferred active languages. A factory that
//! only received roots could not truthfully infer language gating on a bare
//! project, so the context is explicit rather than guessed from a path.
//!
//! [`NativeOfferingService`] builds [`CanonicalOfferingService`], the Native
//! canonical offering: per-kind layered repositories (built-in, every org pack,
//! project) loaded through one lossless read/construction authority, with the
//! reference's schema admission, overlay merge, language gate and declared-id
//! keys. A non-directive key therefore resolves instead of failing closed.
//!
//! # Exact ids
//!
//! [`EffectiveSet::ids`] and [`EffectiveSet::fallback_ids`] are
//! [`PythonString`]s: a service-only id, or an id a pack file declares with a
//! code point a Rust `String` cannot hold, is carried exactly. A caller that
//! wants Rust text asks for the explicit scalar projection
//! ([`EffectiveSet::scalar_ids`]); nothing is dropped silently.
//!
//! It **fails closed** where the reference does. When the org-pack registry is
//! malformed, a declared org pack root is missing, the doctrine service cannot be
//! built (the language inference raises), a repository raises while its keys are
//! read, or an empty set would be returned while built-in artifacts of that kind
//! ship, the entry is `resolved == false` with the reason — never a partial set.
//! `skill` (an absent key means "org-required only") is never resolvable here,
//! and `mission_type_activations` is an activation ledger rather than a corpus.
//!
//! Every unresolved entry except the required-only `skill` also carries
//! [`EffectiveSet::fallback_ids`]: the best-effort union of what is available
//! across every *readable* layer, including the flat mission-type roster.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use sk_charter::{ArtifactKind, CHARTER_KIND_TOKENS, MISSION_TYPE_TOKEN};
use sk_kernel::PythonString;

use crate::identity::{IdentityFailure, IdentityIndex};
use crate::language_scope::{InferRequest, infer_repo_languages};
use crate::ledger::EffectiveWhenAbsent;
use crate::offering::{
    ActivationGate, CanonicalOfferingService, ConstructorAbort, NativeOfferingFailure,
    availability_stems, mission_type_fallback,
};
use crate::plan::EffectiveSet;

/// Reason recorded for a kind whose absent key puts only org-required entries in force (`skill`).
pub const REQUIRED_ONLY_REASON: &str =
    "kind is required-only: an absent key puts only org-required entries in force";

/// Reason recorded for the activation-ledger outlier (`mission_type_activations`).
pub const LEDGER_REASON: &str = "mission_type_activations is an activation ledger, not a corpus: an absent key puts no mission type in force";

/// Prefix of every "the doctrine service cannot be built" reason.
pub const SERVICE_BUILD_PREFIX: &str = "the doctrine service cannot be built: ";

/// Prefix of every "an artifact layer cannot be scanned" reason.
pub const SCAN_FAILURE_PREFIX: &str = "an artifact layer cannot be scanned: ";

/// The `directive` operator token — the one kind whose answer takes no service half.
pub const DIRECTIVE_TOKEN: &str = "directive";

/// What the activation-aware doctrine service offers for one activation key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceKeys {
    /// The loaded offering exposes this mapping: its declared ids.
    Declared(Vec<PythonString>),
    /// The offering exposes no mapping for this key (the reference contributes nothing).
    NoMapping,
    /// A repository raised while its keys were read; the entry is unresolved.
    Failed(String),
    /// A repository raised a Python exception the reference's loader does not
    /// catch. At a property-getter boundary AttributeError becomes NoMapping
    /// (`getattr`'s default); otherwise the strict pass reports unresolved and
    /// the fallback re-read propagates it ([`EffectiveSetError::Fatal`]).
    Aborted(ConstructorAbort),
    /// A typed Native output/capability failure (an activation-warning delivery
    /// that cannot happen, or an unusable start-up set-policy secret).
    ///
    /// The reference has no analogous exception, so this arm must not be routed to
    /// `Failed`/`NoMapping`/unresolved/fallback health: it propagates as
    /// [`EffectiveSetError::Native`].
    Native(NativeOfferingFailure),
}

/// The activation-aware doctrine service one resolution pass reads.
///
/// Mirrors the reference's `ActiveCharterService`: `service_keys` is the keys of
/// the attribute named after the activation key without its `activated_` prefix.
pub trait EffectiveSetService {
    /// Declared ids the loaded offering exposes for `yaml_key`
    /// (`activated_procedures` → the `procedures` repository's keys).
    fn service_keys(&self, yaml_key: &str) -> ServiceKeys;
}

/// Source `_service_keys` reads the property with `getattr(..., None)`. An
/// AttributeError raised inside that getter is swallowed by Python's default,
/// not converted to an unresolved/fatal repository result. Direct repository
/// callers still receive the typed exception; Native failures are never hidden.
fn service_mapping_keys(service: &dyn EffectiveSetService, yaml_key: &str) -> ServiceKeys {
    match service.service_keys(yaml_key) {
        ServiceKeys::Aborted(abort) if abort.python == "AttributeError" => ServiceKeys::NoMapping,
        keys => keys,
    }
}

/// Why an activation-aware offering context could not be built.
///
/// `Unresolvable` is the reference's `_UnresolvableError` class: every pending key
/// becomes unresolved with the reason and the fallback pass runs as before.
/// `Native` is a typed Native capability/start-up failure and must propagate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActivationContextError {
    /// The ordinary activation build/read failure the reference reports unresolved.
    Unresolvable(String),
    /// A typed Native output/capability failure.
    Native(NativeOfferingFailure),
}

impl fmt::Display for ActivationContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unresolvable(reason) => formatter.write_str(reason),
            Self::Native(native) => fmt::Display::fmt(native, formatter),
        }
    }
}

impl std::error::Error for ActivationContextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unresolvable(_) => None,
            Self::Native(native) => Some(native),
        }
    }
}

/// The pack roots one resolution pass reads, resolved once for every requested key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfferingRoots {
    /// The built-in pack root, when it resolves.
    pub built_in: Option<PathBuf>,
    /// Declared org pack roots, in declaration order.
    pub org_roots: Vec<PathBuf>,
    /// The project pack root (`.kittify/charter-packs`), when it is a directory.
    pub project: Option<PathBuf>,
}

/// The repository context one service build reads: the roots **and** what the
/// reference infers from the repository itself.
///
/// `active_languages` is the reference's `infer_repo_languages(repo_root)`
/// projection: `None` is the no-signal state (admit every scoped artifact) and
/// `Some(vec![])` is an explicit empty compiled answer (admit none). The service
/// must never widen a language-filtered repository to `None`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfferingContext {
    /// The repository root the service answers for.
    pub repo_root: PathBuf,
    /// The pack roots in force.
    pub roots: OfferingRoots,
    /// The inferred active languages, in the reference's `list[str] | None` shape.
    pub active_languages: Option<Vec<String>>,
    /// The activation gate this service applies; `None` is the reference's
    /// unfiltered `pack_context=None` form.
    activation: Option<ActivationGate>,
}

impl OfferingContext {
    /// A context over already-inferred languages.
    #[must_use]
    pub fn new(
        repo_root: PathBuf,
        roots: OfferingRoots,
        active_languages: Option<Vec<String>>,
    ) -> Self {
        Self {
            repo_root,
            roots,
            active_languages,
            activation: None,
        }
    }

    /// The activation gate in force, when this context is the canonical
    /// (activation-aware) one.
    #[must_use]
    pub fn activation(&self) -> Option<&ActivationGate> {
        self.activation.as_ref()
    }

    /// Retain context data while starting the new service's runtime caches.
    pub(crate) fn into_fresh_service(mut self) -> Self {
        if let Some(gate) = self.activation.as_mut() {
            gate.reset_service_runtime();
        }
        self
    }

    /// The canonical context over already-inferred languages: the same gate
    /// [`Self::resolve`] builds, without re-reading the repository's languages.
    ///
    /// This is the checked `String` adapter over [`Self::with_activation_typed`]:
    /// the Native arm is rendered for existing callers, it is not the cause
    /// authority.
    ///
    /// # Errors
    ///
    /// The activation-ledger/org-floor failure, rendered as the reference's
    /// service-construction failure.
    pub fn with_activation(
        repo_root: &Path,
        roots: OfferingRoots,
        active_languages: Option<Vec<String>>,
    ) -> Result<Self, String> {
        Self::with_activation_typed(repo_root, roots, active_languages)
            .map_err(|error| error.to_string())
    }

    /// The canonical context with the typed activation-construction failure.
    ///
    /// An ordinary activation read/build failure stays `Unresolvable` (the
    /// reference's `_UnresolvableError`), while a typed Native capability/start-up
    /// failure must reach the effective-set consumer instead of becoming fallback
    /// health.
    ///
    /// # Errors
    ///
    /// [`ActivationContextError::Unresolvable`] for an ordinary activation failure,
    /// [`ActivationContextError::Native`] for the typed Native cause.
    pub fn with_activation_typed(
        repo_root: &Path,
        roots: OfferingRoots,
        active_languages: Option<Vec<String>>,
    ) -> Result<Self, ActivationContextError> {
        let activation = gate_for(repo_root, &roots)?;
        Ok(Self {
            repo_root: repo_root.to_path_buf(),
            roots,
            active_languages,
            activation: Some(activation),
        })
    }

    /// Infer the active languages for `repo_root`, exactly as the reference's
    /// service builder does (`infer_repo_languages(repo_root)`).
    ///
    /// # Errors
    ///
    /// The inference failure, rendered the way the reference renders a raising
    /// builder (`the doctrine service cannot be built: …` is added by the
    /// caller).
    pub fn infer_languages(repo_root: &Path) -> Result<Option<Vec<String>>, String> {
        infer_repo_languages(&InferRequest::new(Some(repo_root)))
            .map(crate::language_scope::InferredLanguages::into_option)
            .map_err(|error| error.to_string())
    }

    /// The context for `repo_root`: strict pack roots plus inferred languages.
    ///
    /// This is the one public way to obtain the context the canonical service
    /// reads, so a caller (or a test) can never assemble a context the resolver
    /// would not have built.
    ///
    /// # Errors
    ///
    /// The strict root resolution or the language inference failure.
    pub fn resolve(repo_root: &Path) -> Result<Self, String> {
        let roots = strict_roots(repo_root)?;
        Self::with_roots(repo_root, roots)
    }

    /// The context for already-resolved roots plus the inferred languages.
    ///
    /// Checked `String` adapter over [`Self::with_roots_typed`].
    ///
    /// # Errors
    ///
    /// The language inference failure or the activation build failure.
    pub fn with_roots(repo_root: &Path, roots: OfferingRoots) -> Result<Self, String> {
        Self::with_roots_typed(repo_root, roots).map_err(|error| error.to_string())
    }

    /// [`Self::with_roots`] with the typed activation-construction failure.
    ///
    /// # Errors
    ///
    /// The language inference failure (rendered as the reference's
    /// service-construction reason) or [`ActivationContextError::Native`].
    pub fn with_roots_typed(
        repo_root: &Path,
        roots: OfferingRoots,
    ) -> Result<Self, ActivationContextError> {
        let active_languages =
            Self::infer_languages(repo_root).map_err(ActivationContextError::Unresolvable)?;
        Self::with_activation_typed(repo_root, roots, active_languages)
    }
}

/// Builds the doctrine service from the repository context.
///
/// This is the native counterpart of `build_active_charter_service(repo_root)`:
/// it is handed the context the pass already resolved, so the service and the
/// scans can never disagree about which packs and languages are in force. `Err`
/// carries the reason the reference would report as `the doctrine service cannot
/// be built`.
pub trait OfferingServiceFactory {
    /// Build the service for `context`, or return the build-failure reason.
    fn build(&self, context: &OfferingContext) -> Result<Box<dyn EffectiveSetService>, String>;
}

/// The canonical Native offering service factory.
///
/// It builds [`CanonicalOfferingService`] over the context's roots and inferred
/// languages — the Native `build_active_charter_service`. Loading is per kind and
/// lazy, exactly like the reference's service properties, so this constructor
/// itself does no repository I/O and cannot fail.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeOfferingService;

impl OfferingServiceFactory for NativeOfferingService {
    fn build(&self, context: &OfferingContext) -> Result<Box<dyn EffectiveSetService>, String> {
        Ok(Box::new(CanonicalOfferingService::new(context.clone())))
    }
}

/// The one ready-made factory: [`NativeOfferingService`].
pub const NATIVE_OFFERING_SERVICE: NativeOfferingService = NativeOfferingService;

/// An activation key outside the canonical charter kind universe.
///
/// The reference raises `ValueError` with the same text; nothing is resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnknownActivationKey {
    /// The key the caller asked for.
    pub key: String,
    /// The valid activation keys, sorted.
    pub valid: Vec<String>,
}

impl UnknownActivationKey {
    /// Capture the offending key against the canonical vocabulary.
    #[must_use]
    pub fn new(key: &str) -> Self {
        Self {
            key: key.to_owned(),
            valid: valid_activation_keys(),
        }
    }
}

impl fmt::Display for UnknownActivationKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Unknown activation key {}. Valid keys: [{}]",
            python_repr(&self.key),
            self.valid
                .iter()
                .map(|key| python_repr(key))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl std::error::Error for UnknownActivationKey {}

/// Why one resolution pass could not answer at all.
///
/// The reference's `resolve_effective_sets` raises `ValueError` for an unknown
/// key (before any I/O) and lets a constructor abort from the fallback's service
/// re-read propagate. An ordinary unresolvable key is **not** an error: it is an
/// entry with `resolved == false` and its reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectiveSetError {
    /// The requested key is outside the canonical kind universe.
    UnknownKey(UnknownActivationKey),
    /// A Python exception the reference's artifact loader does not catch.
    Fatal(ConstructorAbort),
    /// A typed Native output/capability failure: never a source exception, never an
    /// ordinary unresolved/fallback-health answer.
    Native(NativeOfferingFailure),
}

impl EffectiveSetError {
    /// Whether this failure is the typed Native arm.
    #[must_use]
    pub fn is_native(&self) -> bool {
        matches!(self, Self::Native(_))
    }

    /// The typed Native cause, when this failure is one.
    #[must_use]
    pub fn native(&self) -> Option<&NativeOfferingFailure> {
        match self {
            Self::Native(native) => Some(native),
            Self::UnknownKey(_) | Self::Fatal(_) => None,
        }
    }
}

impl From<UnknownActivationKey> for EffectiveSetError {
    fn from(error: UnknownActivationKey) -> Self {
        Self::UnknownKey(error)
    }
}

impl fmt::Display for EffectiveSetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKey(error) => fmt::Display::fmt(error, formatter),
            Self::Fatal(abort) => fmt::Display::fmt(abort, formatter),
            Self::Native(native) => fmt::Display::fmt(native, formatter),
        }
    }
}

impl std::error::Error for EffectiveSetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::UnknownKey(error) => Some(error),
            Self::Fatal(abort) => Some(abort),
            Self::Native(native) => Some(native),
        }
    }
}

/// Carry the same two-arm identity failure into this module's existing authority.
///
/// A Source abort stays the reference's [`EffectiveSetError::Fatal`]; a Native
/// start-up policy failure keeps its typed arm and never becomes unresolved health.
fn identity_effective_error(failure: IdentityFailure) -> EffectiveSetError {
    match failure {
        IdentityFailure::Constructor(abort) => EffectiveSetError::Fatal(abort),
        IdentityFailure::Native(native) => EffectiveSetError::Native(native),
    }
}

/// Every valid activation key, sorted (the reference's `sorted(YAML_KEY_MAP.values())`).
#[must_use]
pub fn valid_activation_keys() -> Vec<String> {
    let mut keys: Vec<String> = CHARTER_KIND_TOKENS
        .iter()
        .map(|token| yaml_key_for_token(token))
        .collect();
    keys.sort();
    keys
}

/// The `config.yaml` activation key for a charter kind *token*.
///
/// `mission-type` is the documented outlier (`mission_type_activations`); every
/// other kind follows `activated_<plural>`, with the plural taken from the
/// canonical [`ArtifactKind`] rather than a hand-maintained table.
#[must_use]
pub fn yaml_key_for_token(token: &str) -> String {
    if token == MISSION_TYPE_TOKEN {
        return "mission_type_activations".to_owned();
    }
    match ArtifactKind::from_operator_token(token) {
        Ok(kind) => format!("activated_{}", kind.plural()),
        // Unknown tokens are refused by `token_for` before this point; the
        // fallback keeps this helper total without inventing a key.
        Err(_) => String::new(),
    }
}

/// The kind operator token for an activation key; `None` for an unknown key.
#[must_use]
pub fn token_for(yaml_key: &str) -> Option<&'static str> {
    CHARTER_KIND_TOKENS
        .iter()
        .copied()
        .find(|token| yaml_key_for_token(token) == yaml_key)
}

/// Return what is effective for each of `yaml_keys` while that key is absent.
///
/// One entry per distinct key, in request order (duplicates are resolved once),
/// mirroring the reference's `dict.fromkeys` contract.
///
/// `resolved == false` (with [`EffectiveSet::reason`]) means the caller must
/// leave the key absent and report it — never write a bare list. Such an entry
/// carries [`EffectiveSet::fallback_ids`] except for the required-only `skill`.
///
/// # Errors
///
/// [`UnknownActivationKey`] for a key outside the canonical kind universe; no
/// repository I/O has happened at that point, exactly as in the reference.
pub fn resolve_effective_sets(
    repo_root: &Path,
    yaml_keys: &[&str],
    factory: &dyn OfferingServiceFactory,
) -> Result<Vec<EffectiveSet>, EffectiveSetError> {
    let mut requested: Vec<RequestedKey> = Vec::with_capacity(yaml_keys.len());
    for key in yaml_keys {
        if requested.iter().any(|entry| entry.yaml_key == *key) {
            continue;
        }
        let Some(token) = token_for(key) else {
            return Err(EffectiveSetError::UnknownKey(UnknownActivationKey::new(
                key,
            )));
        };
        requested.push(RequestedKey {
            yaml_key: (*key).to_owned(),
            yaml_name: yaml_key_for_token(token),
            token,
        });
    }

    let mut results: Vec<Option<EffectiveSet>> = vec![None; requested.len()];
    let mut pending: Vec<usize> = Vec::new();
    for (index, entry) in requested.iter().enumerate() {
        if entry.token == MISSION_TYPE_TOKEN {
            results[index] = Some(unresolved(entry, LEDGER_REASON.to_owned()));
            continue;
        }
        let kind = ArtifactKind::from_operator_token(entry.token).expect("charter kind token");
        if EffectiveWhenAbsent::for_kind(kind) == EffectiveWhenAbsent::Required {
            results[index] = Some(unresolved(entry, REQUIRED_ONLY_REASON.to_owned()));
        } else {
            pending.push(index);
        }
    }

    let mut offering: Option<Offering> = None;
    if !pending.is_empty() {
        let with_service = pending
            .iter()
            .any(|index| requested[*index].token != DIRECTIVE_TOKEN);
        match Offering::load(repo_root, with_service, factory) {
            Err(LoadFailure::Unresolvable(reason)) => {
                for index in pending {
                    results[index] = Some(unresolved(&requested[index], reason.clone()));
                }
            }
            Err(LoadFailure::Native(native)) => return Err(EffectiveSetError::Native(native)),
            Ok(loaded) => {
                for index in &pending {
                    results[*index] = Some(loaded.resolve_one(&requested[*index])?);
                }
                offering = Some(loaded);
            }
        }
    }

    let needing_fallback: Vec<usize> = results
        .iter()
        .enumerate()
        .filter(|(_, entry)| match entry {
            Some(set) => !set.resolved && set.reason.as_deref() != Some(REQUIRED_ONLY_REASON),
            None => false,
        })
        .map(|(index, _)| index)
        .collect();
    if !needing_fallback.is_empty() {
        let fallback = Fallback::load(repo_root, offering.as_ref(), &requested, &needing_fallback)
            .map_err(EffectiveSetError::Native)?;
        for index in needing_fallback {
            let entry = results[index].take().expect("entry resolved above");
            let fallback_ids = fallback.ids_for(&requested[index])?;
            results[index] = Some(EffectiveSet {
                fallback_ids,
                ..entry
            });
        }
    }

    Ok(results
        .into_iter()
        .map(|entry| entry.expect("every requested key has an entry"))
        .collect())
}

/// One deduplicated requested key: the caller's spelling, its canonical key and its token.
struct RequestedKey {
    yaml_key: String,
    yaml_name: String,
    token: &'static str,
}

fn unresolved(entry: &RequestedKey, reason: String) -> EffectiveSet {
    EffectiveSet {
        kind: entry.token.to_owned(),
        yaml_key: entry.yaml_key.clone(),
        ids: Vec::new(),
        fallback_ids: Vec::new(),
        resolved: false,
        reason: Some(reason),
    }
}

/// The roots, the identity index, the context and the service of one resolution pass.
struct Offering {
    roots: OfferingRoots,
    identity: IdentityIndex,
    service: Option<Box<dyn EffectiveSetService>>,
}

/// Why the one offering pass could not load.
///
/// `Unresolvable` mirrors the reference's `_UnresolvableError` (every pending key
/// becomes unresolved and the fallback runs), while `Native` is the typed Native
/// capability/start-up failure that the reference has no class for and that must
/// reach the caller instead of becoming health.
enum LoadFailure {
    /// The ordinary offering/roots/service build failure.
    Unresolvable(String),
    /// A typed Native output/capability failure.
    Native(NativeOfferingFailure),
}

impl Offering {
    /// Load the roots (and, when any pending key needs it, the service) once.
    ///
    /// A failure here is the reference's `_UnresolvableError`: every pending key
    /// becomes unresolved with this reason, never a partial set. A typed Native
    /// failure is not that class and propagates.
    fn load(
        repo_root: &Path,
        with_service: bool,
        factory: &dyn OfferingServiceFactory,
    ) -> Result<Self, LoadFailure> {
        let roots = strict_roots(repo_root).map_err(LoadFailure::Unresolvable)?;
        let Some(built_in) = roots.built_in.as_deref() else {
            return Err(LoadFailure::Unresolvable(format!(
                "the built-in pack root cannot be resolved for {}",
                repo_root.display()
            )));
        };
        let identity =
            IdentityIndex::from_roots(built_in, &roots.org_roots, roots.project.as_deref());
        let service = if with_service {
            let context = match OfferingContext::with_roots_typed(repo_root, roots.clone()) {
                Ok(context) => context,
                Err(ActivationContextError::Unresolvable(reason)) => {
                    return Err(LoadFailure::Unresolvable(format!(
                        "{SERVICE_BUILD_PREFIX}{reason}"
                    )));
                }
                Err(ActivationContextError::Native(native)) => {
                    return Err(LoadFailure::Native(native));
                }
            };
            Some(
                factory
                    .build(&context)
                    .map_err(|reason| format!("{SERVICE_BUILD_PREFIX}{reason}"))
                    .map_err(LoadFailure::Unresolvable)?,
            )
        } else {
            None
        };
        Ok(Self {
            roots,
            identity,
            service,
        })
    }

    /// Resolve one key against the loaded offering, in activation-stem spelling.
    ///
    /// # Errors
    ///
    /// A typed Native failure from the service read (the reference has no class for
    /// it and it must not become unresolved/fallback health).
    fn resolve_one(&self, entry: &RequestedKey) -> Result<EffectiveSet, EffectiveSetError> {
        let kind = ArtifactKind::from_operator_token(entry.token).expect("charter kind token");
        let mut ids: BTreeSet<PythonString> = availability_stems(kind, &self.roots)
            .into_iter()
            .map(|stem| PythonString::from_scalars(&stem))
            .collect();
        if entry.token != DIRECTIVE_TOKEN {
            let Some(service) = self.service.as_deref() else {
                return Ok(unresolved(
                    entry,
                    format!("{SERVICE_BUILD_PREFIX}no offering service was built for this pass"),
                ));
            };
            match service_mapping_keys(service, &entry.yaml_name) {
                ServiceKeys::Declared(declared) => {
                    for declared_id in declared {
                        let stem = match self.map_declared(kind, &declared_id) {
                            Ok(stem) => stem,
                            // The reference's `_as_stems` runs inside `_resolve_one`'s
                            // `try`, so a scan abort while mapping a declared id leaves
                            // this key unresolved instead of raising from the strict pass.
                            Err(IdentityFailure::Constructor(abort)) => {
                                return Ok(unresolved(
                                    entry,
                                    format!("{SCAN_FAILURE_PREFIX}{abort}"),
                                ));
                            }
                            // A Native start-up policy failure has no Source analogue:
                            // it propagates instead of becoming unresolved health.
                            Err(IdentityFailure::Native(native)) => {
                                return Err(EffectiveSetError::Native(native));
                            }
                        };
                        ids.insert(stem);
                    }
                }
                ServiceKeys::NoMapping => {}
                ServiceKeys::Failed(cause) => {
                    return Ok(unresolved(entry, format!("{SCAN_FAILURE_PREFIX}{cause}")));
                }
                ServiceKeys::Aborted(abort) => {
                    // The reference's `_resolve_one` catches this and reports the
                    // key unresolved; the fallback phase re-reads and propagates.
                    return Ok(unresolved(entry, format!("{SCAN_FAILURE_PREFIX}{abort}")));
                }
                // A Native output/capability failure is not a Source exception and
                // has no unresolved/fallback analogue: it propagates untouched.
                ServiceKeys::Native(native) => return Err(EffectiveSetError::Native(native)),
            }
        }
        if ids.is_empty() && self.built_in_ships(kind) {
            return Ok(unresolved(
                entry,
                format!(
                    "empty effective set while built-in artifacts of {} ship",
                    entry.token
                ),
            ));
        }
        Ok(EffectiveSet {
            kind: entry.token.to_owned(),
            yaml_key: entry.yaml_key.clone(),
            ids: ids.into_iter().collect(),
            fallback_ids: Vec::new(),
            resolved: true,
            reason: None,
        })
    }

    /// The activation stem for a service-declared id; an unknown id is kept exactly.
    ///
    /// # Errors
    ///
    /// The queried kind's scan abort (the index is lazy per kind, so an unused
    /// kind's abort is never surfaced here).
    fn map_declared(
        &self,
        kind: ArtifactKind,
        declared_id: &PythonString,
    ) -> Result<PythonString, IdentityFailure> {
        Ok(self
            .identity
            .try_stem_exact(kind, declared_id)?
            .unwrap_or_else(|| declared_id.clone()))
    }

    /// Whether the built-in pack ships any artifact file of `kind`.
    fn built_in_ships(&self, kind: ArtifactKind) -> bool {
        if !kind.has_built_in_content_dir() {
            return false;
        }
        let Some(root) = self.roots.built_in.as_ref() else {
            return false;
        };
        let directory = root.join(kind.plural());
        directory.is_dir() && has_yaml_file(&directory)
    }
}

/// The best-effort readable chain, used only for [`EffectiveSet::fallback_ids`].
struct Fallback<'a> {
    roots: OfferingRoots,
    identity: IdentityIndex,
    service: Option<ServiceHandle<'a>>,
}

/// The fallback's service: the strict pass's own service when it has one, else the
/// canonical rebuild the reference performs over the readable roots.
enum ServiceHandle<'a> {
    Borrowed(&'a dyn EffectiveSetService),
    Owned(Box<dyn EffectiveSetService>),
}

impl ServiceHandle<'_> {
    fn get(&self) -> &dyn EffectiveSetService {
        match self {
            Self::Borrowed(service) => *service,
            Self::Owned(service) => service.as_ref(),
        }
    }
}

impl<'a> Fallback<'a> {
    /// Build the readable-layer index and, when a needed kind takes one, the service.
    ///
    /// Best effort, unlike the strict pass: a missing or malformed declared org
    /// pack contributes no org layer instead of failing the fallback, and an ordinary
    /// service build failure leaves the fallback without one (the reference's
    /// `_fallback_service` catches `Exception`). A typed Native failure is not that
    /// class: it is returned so the caller cannot report fallback health.
    ///
    /// # Errors
    ///
    /// The typed Native activation-construction failure.
    fn load(
        repo_root: &Path,
        offering: Option<&'a Offering>,
        requested: &[RequestedKey],
        needing: &[usize],
    ) -> Result<Self, NativeOfferingFailure> {
        let roots = match offering {
            Some(loaded) => loaded.roots.clone(),
            None => lenient_roots(repo_root),
        };
        let identity = match roots.built_in.as_deref() {
            Some(built_in) => {
                IdentityIndex::from_roots(built_in, &roots.org_roots, roots.project.as_deref())
            }
            // Without a built-in root the canonical index cannot be built; the
            // fallback then contributes nothing rather than a fabricated layer.
            None => IdentityIndex::default(),
        };
        let needs_service = needing.iter().any(|index| {
            let token = requested[*index].token;
            token != DIRECTIVE_TOKEN && token != MISSION_TYPE_TOKEN
        });
        let service = match offering {
            Some(loaded) => loaded.service.as_deref().map(ServiceHandle::Borrowed),
            None if needs_service => match OfferingContext::infer_languages(repo_root) {
                Ok(active_languages) => {
                    match OfferingContext::with_activation_typed(
                        repo_root,
                        roots.clone(),
                        active_languages,
                    ) {
                        Ok(context) => NATIVE_OFFERING_SERVICE
                            .build(&context)
                            .ok()
                            .map(ServiceHandle::Owned),
                        Err(ActivationContextError::Unresolvable(_)) => None,
                        Err(ActivationContextError::Native(native)) => return Err(native),
                    }
                }
                Err(_) => None,
            },
            None => None,
        };
        Ok(Self {
            roots,
            identity,
            service,
        })
    }

    /// The union of ids available across every readable layer, as activation stems.
    ///
    /// # Errors
    ///
    /// The constructor abort a population re-read raises (the reference's
    /// `_with_fallbacks` → `_fallback_ids` has no `except` around the service
    /// read, so it propagates out of `resolve_effective_sets`), or the typed Native
    /// service failure (which must not degrade into fallback health).
    fn ids_for(&self, entry: &RequestedKey) -> Result<Vec<PythonString>, EffectiveSetError> {
        if entry.token == MISSION_TYPE_TOKEN {
            return Ok(mission_type_fallback(&self.roots));
        }
        let kind = ArtifactKind::from_operator_token(entry.token).expect("charter kind token");
        let mut ids: BTreeSet<PythonString> = availability_stems(kind, &self.roots)
            .into_iter()
            .map(|stem| PythonString::from_scalars(&stem))
            .collect();
        if entry.token != DIRECTIVE_TOKEN
            && let Some(service) = self.service.as_ref().map(ServiceHandle::get)
        {
            match service_mapping_keys(service, &entry.yaml_name) {
                ServiceKeys::Declared(declared) => {
                    for declared_id in declared {
                        // The reference's `_fallback_ids` has no `except` around this
                        // read, so a scan abort propagates out of `resolve_effective_sets`.
                        let stem = self
                            .identity
                            .try_stem_exact(kind, &declared_id)
                            .map_err(identity_effective_error)?;
                        ids.insert(stem.unwrap_or(declared_id));
                    }
                }
                ServiceKeys::NoMapping | ServiceKeys::Failed(_) => {}
                ServiceKeys::Aborted(abort) => return Err(EffectiveSetError::Fatal(abort)),
                ServiceKeys::Native(native) => return Err(EffectiveSetError::Native(native)),
            }
        }
        Ok(ids.into_iter().collect())
    }
}

/// The activation gate for already-resolved roots (the reference's
/// `PackContext.from_config` over the pack set the pass resolved).
///
/// An ordinary activation failure keeps the reference's resolvable-error class; a
/// typed Native capability/start-up failure is returned as its own arm so the
/// effective-set pass can propagate it instead of reporting unresolved/fallback
/// health.
fn gate_for(
    repo_root: &Path,
    roots: &OfferingRoots,
) -> Result<ActivationGate, ActivationContextError> {
    let Some(built_in) = roots.built_in.as_deref() else {
        return Err(ActivationContextError::Unresolvable(
            "the built-in pack root cannot be resolved".to_owned(),
        ));
    };
    ActivationGate::try_from_project(
        repo_root,
        built_in,
        &roots.org_roots,
        roots.project.as_deref(),
    )
    .map_err(|error| match error {
        crate::ledger::ActivationReadError::Native(native) => {
            ActivationContextError::Native(native)
        }
        other => ActivationContextError::Unresolvable(other.to_string()),
    })
}

/// Resolve every declared org pack root in declaration order, failing closed.
///
/// `sk-charter`'s resolver is already strict about the org chain
/// (`CharterError::OrgPackMissing` for a declared root that is not a directory,
/// a config read error for a malformed registry), so the reference's
/// `require_declared_org_roots` posture needs no second registry reader here —
/// only its two refusal sentences are rendered.
fn strict_roots(repo_root: &Path) -> Result<OfferingRoots, String> {
    let built_in = crate::org_charter::built_in_search(repo_root)
        .find_root()
        .ok();
    let config = sk_charter::load_project_config(repo_root, &sk_charter::Limits::default())
        .map_err(|error| format!("the org pack registry cannot be read: {error}"))?;
    let expansion = sk_charter::PathExpansion::from_env();
    let search = crate::org_charter::built_in_search(repo_root);
    let roots = sk_charter::PackRoots::resolve_source(repo_root, &config, &expansion, Some(&search))
        .map_err(|error| match error {
            sk_charter::CharterError::OrgPackMissing { name, root, .. } => format!(
                "declared org pack {} is not fetched: its root {} is not a directory; run `spec-kitty charter fetch` to fetch it",
                python_repr(&name),
                root.display()
            ),
            other => format!("the org pack registry cannot be read: {other}"),
        })?;
    Ok(OfferingRoots {
        built_in,
        org_roots: roots
            .organizations
            .iter()
            .map(|pack| pack.root.clone())
            .collect(),
        project: roots.project,
    })
}

/// Readable fallback tiers through the canonical pack-root authority.
/// An unreadable org registry contributes no org root, NOT no project root.
fn lenient_roots(repo_root: &Path) -> OfferingRoots {
    let built_in = crate::org_charter::built_in_search(repo_root)
        .find_root()
        .ok();
    let config = sk_charter::load_project_config(repo_root, &sk_charter::Limits::default()).ok();
    let expansion = sk_charter::PathExpansion::from_env();
    let search = crate::org_charter::built_in_search(repo_root);
    let resolve = |config| {
        sk_charter::PackRoots::resolve_existing_source(repo_root, config, &expansion, Some(&search))
    };
    match resolve(config.as_ref()).or_else(|_| resolve(None)) {
        Ok(roots) => OfferingRoots {
            built_in,
            org_roots: roots
                .organizations
                .iter()
                .map(|pack| pack.root.clone())
                .collect(),
            project: roots.project,
        },
        // A registry that still cannot be read contributes no org layer; the
        // project and built-in halves stay readable.
        Err(_) => OfferingRoots {
            built_in,
            org_roots: Vec::new(),
            project: None,
        },
    }
}

/// Whether any `*.yaml` file exists under `directory` (the reference's `rglob`).
fn has_yaml_file(directory: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if has_yaml_file(&path) {
                return true;
            }
        } else if path
            .extension()
            .is_some_and(|extension| extension == "yaml")
        {
            return true;
        }
    }
    false
}

/// The reference's Python `repr` for these (scalar) spellings.
fn python_repr(text: &str) -> String {
    PythonString::from_scalars(text).repr()
}
