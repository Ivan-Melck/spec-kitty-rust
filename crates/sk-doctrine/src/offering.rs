//! The canonical active offering service (`charter.activation.active_charter_service_builder`
//! + `charter.offering.service`), plus flat mission-type availability.
//!
//! This is the Native counterpart of
//! `build_active_charter_service(repo_root)`: one service over the built-in pack,
//! every declared org pack and the project pack root that exposes, per activation
//! kind, the **declared ids its loaded repository holds**. The effective-set seam
//! unions those with the layer scan, so a non-directive key resolves exactly as
//! the reference resolves it instead of failing closed.
//!
//! # Layer and admission rules (reference → here)
//!
//! * **Layers.** Built-in (`packs/built-in/<plural>`, recursive), each declared
//!   org root (`<root>/<plural>`, recursive, in declaration order) and the
//!   project pack root (`.kittify/charter-packs/<dir>`, recursive). Mission step
//!   contracts are the documented outlier: their built-in layer is
//!   `packs/built-in/missions/built_in_step_contracts` and the repository's
//!   overlay directories are still `<root>/mission_step_contracts`
//!   (`MissionStepContractRepository._default_built_in_dir`).
//! * **One read/construction authority.** Every document is read through
//!   [`ArtifactDocument`] (the crate's lossless transport, the same one the
//!   identity index uses) and a declared name is built with Python
//!   `str(value).strip()` — so a `null`, list, mapping, bool or numeric id is
//!   the same exact `str` the reference builds, never a dropped id.
//! * **Activation gate.** Every gated mapping is filtered through
//!   [`ActivationGate`] — the reference's `ActiveCharterService._gated`: the
//!   effective deny `D*` per kind and the three-state allow set, with allow
//!   tokens resolved through the identity index. A key absent from the ledger is
//!   still deny-netted, and an explicitly empty allow list admits nothing.
//! * **Schema admission.** A document is loaded only when the reference's whole
//!   Pydantic model accepts it ([`crate::offering::model`]): nested sub-models,
//!   `extra="forbid"` key sets, literals, patterns, `min_length` and the
//!   model validators, plus the `applies_to_languages` scope through the
//!   canonical matcher. A document that fails is skipped with the reference's
//!   warning semantics (not fatal); the Python exceptions the loader does not
//!   catch abort the load ([`ConstructorAbort`]).
//! * **Repository keys are authored keys.** A built-in item is filed under its
//!   model `id` and an overlay under the raw `data.get("id")` value; only the
//!   availability and identity phases strip, exactly as the reference does.
//! * **Overlay merge.** An overlay document whose id matches a built-in item
//!   merges field-by-field onto the *built-in* item (the reference's
//!   `_merge_overlay_item`), so a project override that omits a field inherits
//!   the built-in value rather than the org value.
//! * **Agent profiles** use the crate's existing canonical
//!   [`ProfileRepository`], loaded **with** the inferred active languages (never
//!   a `None` widening), and contribute its loaded ids.
//!
//! # Mission types are a flat roster, not an artifact kind
//!
//! `mission_type_activations` is never resolved (it is an activation ledger), but
//! an unresolved mission-type entry still carries `fallback_ids`: the reference
//! scans `missions/mission_types` (built-in), every declared org root's
//! `mission_types` and `.kittify/missions/mission_types` with a schema-validating,
//! loud-fail, flat scan. [`mission_type_fallback`] reproduces that, including its
//! per-root-map failure posture: one unreadable directory drops that map's whole
//! contribution and keeps the rest.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_yaml_ng::Value;
use sk_charter::ArtifactKind;
use sk_kernel::PythonString;
use sk_kernel::python_yaml::{NodeRef, ObjectGraph};

use crate::effective_set::{EffectiveSetService, OfferingContext, OfferingRoots, ServiceKeys};
use crate::identity::python_strip;
use crate::profile::{ProfileRepository, applies_to_languages_match_exact};

pub(crate) mod gate;
pub(crate) mod include_access;
pub(crate) mod model;
pub(crate) mod prevalidation;
pub(crate) mod value;

pub use gate::{ActivationGate, GateFailure, WarningLog};
pub(crate) use value::{ArtifactDocument, exact_merged_graph};

/// The org-tier mission-type roster directory (`<pack>/mission_types`, CL-005).
pub const ORG_MISSION_TYPES_SUBDIR: &str = "mission_types";

/// The project-tier mission-type roster tail under `.kittify` (CL-005).
pub const PROJECT_MISSION_TYPES_RELATIVE_TO_KITTIFY: [&str; 2] = ["missions", "mission_types"];

/// The built-in mission-tier directory holding step contracts.
const BUILT_IN_STEP_CONTRACTS: &str = "missions/built_in_step_contracts";

/// The built-in mission-tier directory holding mission types.
const BUILT_IN_MISSION_TYPES: &str = "missions/mission_types";

/// A Python exception the reference's per-file `except (YAMLError,
/// ValidationError, OSError)` does **not** catch.
///
/// `BaseArtifactRepository` catches exactly those three (and `ConstructorError`
/// is a `YAMLError`), so everything else — the bare `KeyError` from an invalid
/// `!!bool` spelling, a `ValueError` from `int()`/`float()`/date construction,
/// the `TypeError` from an unhashable overlay id, the `AssertionError` from a
/// duplicate `!!omap` key — aborts the whole repository load. The effective-set
/// pass then either reports one unresolvable key (the strict pass catches it) or
/// propagates it (the fallback's service re-read, which has no catch).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConstructorAbort {
    /// The Python exception name the reference's traceback ends with.
    pub python: &'static str,
    /// Its message.
    pub message: String,
}

impl std::fmt::Display for ConstructorAbort {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.python, self.message)
    }
}

impl std::error::Error for ConstructorAbort {}

/// A lazy memo that publishes only a **completed** construction.
///
/// One publication rule for every lazy cache this module and the identity index
/// keep. The reference's lazy service properties assign their entry only after
/// the repository constructor returned (`self._cache["tactics"] =
/// TacticRepository(...)`) and `IdentityIndex._table` assigns the kind table
/// only after `_scan` returned, so a construction that raises leaves nothing
/// cached and the next call retries it. A stored value is then the instance's
/// answer for the rest of its life — a successful snapshot is never invalidated
/// by a later disk change (a new instance reads the new state).
///
/// The guard holds no data and only serialises publishers, so a panic while
/// building leaves the slot empty and retryable instead of poisoning an
/// invariant. This is not a new lock authority: it is the standard library
/// `OnceLock` plus the smallest possible publish race guard, and there is no
/// `unsafe`, dependency or FFI.
pub(crate) struct SuccessOnce<T> {
    slot: OnceLock<T>,
    publisher: Mutex<()>,
}

impl<T> SuccessOnce<T> {
    pub(crate) const fn new() -> Self {
        Self {
            slot: OnceLock::new(),
            publisher: Mutex::new(()),
        }
    }

    /// The published value, or one attempt at `build` whose success is published.
    ///
    /// # Errors
    ///
    /// The build's own error, returned unchanged and **not** memoised.
    pub(crate) fn get_or_try_init<E>(&self, build: impl FnOnce() -> Result<T, E>) -> Result<&T, E> {
        if let Some(value) = self.slot.get() {
            return Ok(value);
        }
        let guard = self
            .publisher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(value) = self.slot.get() {
            return Ok(value);
        }
        let value = build()?;
        let published = self.slot.set(value);
        debug_assert!(
            published.is_ok(),
            "the publisher guard serialises publications"
        );
        drop(guard);
        Ok(self.slot.get().expect("a value was just published"))
    }
}

impl<T> Default for SuccessOnce<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> Clone for SuccessOnce<T> {
    fn clone(&self) -> Self {
        let slot = OnceLock::new();
        if let Some(value) = self.slot.get() {
            let _ = slot.set(value.clone());
        }
        Self {
            slot,
            publisher: Mutex::new(()),
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for SuccessOnce<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SuccessOnce")
            .field("slot", &self.slot)
            .finish_non_exhaustive()
    }
}

/// A Native offering/runtime capability failure: never a Source Python exception.
///
/// The reference's activation gate cannot reach these states. An unusable
/// `PYTHONHASHSEED` and an unusable stdio backend kill CPython before user code (so
/// the reference never constructs a set and never calls `logging`), and a
/// Source-valid codec/error handler this native composition lacks has no reference
/// failure to imitate. They are therefore typed Native causes carried through the
/// existing `OfferingLoadError` -> `ServiceKeys` -> `EffectiveSetError` seam: never
/// an ordinary unresolved/`Failed`/fallback-health answer, never a fake Source
/// class, fake IO, panic or text-only reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeOfferingFailure {
    /// The activation set order needs a usable `PYTHONHASHSEED`.
    ///
    /// CPython refuses to start for an invalid value, so there is no Source
    /// exception class or message to match; the phase/path and the original Kernel
    /// [`sk_kernel::python_yaml::HashSeedError`] are retained as data.
    ActivationSetOrder {
        /// The read phase that reached the set construction.
        phase: String,
        /// The file or repository root the phase was handling.
        path: PathBuf,
        /// The original Kernel start-up cause (byte-exact fatal text).
        cause: sk_kernel::python_yaml::HashSeedError,
    },
    /// The Source-default warning transport cannot deliver an activation warning.
    ///
    /// `log` is the gate's actual delivery evidence up to and including the escape.
    ActivationWarningDelivery {
        /// The original stream cause (a real invalid start-up value or a
        /// Source-valid backend this composition does not carry).
        cause: Box<sk_kernel::python_stream::StreamError>,
        /// The gate's delivery outcomes and escape evidence for this attempt.
        log: Box<WarningLog>,
    },
    /// A Source diagnostic cannot pass the current scalar-only exception carrier.
    /// This is a required Native capability gap, not a Source-invalid document.
    DiagnosticProjection {
        /// The original native filename, without lossy display conversion.
        path: PathBuf,
        /// The diagnostic fragment that needs the missing representation.
        part: &'static str,
        /// The exact original fragment, never replacement text or repr-as-data.
        exact: PythonString,
        /// The original checked projection failure (the first non-scalar unit).
        codepoint: u32,
    },
    /// The interpreter's integer-conversion policy cannot be resolved.
    ///
    /// An invalid `PYTHONINTMAXSTRDIGITS` kills CPython before any business code
    /// runs, so there is no Source Python business exception to reproduce; this keeps the
    /// phase/path and the original canonical Kernel policy error as data instead of
    /// inventing a class, an unlimited policy or a healthy skip.
    IntegerTextPolicy {
        /// The read phase that needed the decimal conversion ceiling.
        phase: &'static str,
        /// The file the phase was handling.
        path: PathBuf,
        /// The original canonical start-up policy error, boxed so this start-up
        /// boundary does not inflate every `Result` carrying the enum.
        cause: Box<sk_kernel::python_json::PythonJsonError>,
    },
    /// The existing filesystem codec cannot produce a Source diagnostic path.
    DiagnosticPathDecode {
        /// The original native filename.
        path: PathBuf,
        /// The original canonical filesystem-decoder cause.
        cause: sk_kernel::python_os_encoding::OsDecodeError,
    },
}

impl std::fmt::Display for NativeOfferingFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ActivationSetOrder { phase, path, cause } => write!(
                formatter,
                "{phase} is unavailable at {}: native start-up policy ({})",
                path.display(),
                cause.to_string().lines().next().unwrap_or("HashSeedError")
            ),
            Self::ActivationWarningDelivery { cause, .. } => write!(
                formatter,
                "activation warning delivery is unavailable in this native stream \
                 composition: {cause}"
            ),
            Self::DiagnosticProjection {
                part, codepoint, ..
            } => write!(
                formatter,
                "native Source diagnostic representation is unavailable: {part} contains U+{codepoint:04X}"
            ),
            Self::DiagnosticPathDecode { cause, .. } => write!(
                formatter,
                "native Source diagnostic path decoding is unavailable: {cause}"
            ),
            Self::IntegerTextPolicy { phase, path, cause } => write!(
                formatter,
                "the interpreter integer conversion policy is unavailable during {phase} at {}: {cause}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for NativeOfferingFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ActivationSetOrder { cause, .. } => Some(cause),
            Self::ActivationWarningDelivery { cause, .. } => Some(cause.as_ref()),
            Self::DiagnosticProjection { .. } => None,
            Self::DiagnosticPathDecode { cause, .. } => Some(cause),
            Self::IntegerTextPolicy { cause, .. } => Some(cause.as_ref()),
        }
    }
}

/// Why the canonical offering service could not expose a mapping for one kind.
///
/// The reference's repositories are lazy properties: a repository that raises
/// during `_load` surfaces while the effective-set module asks for its keys, and
/// the reference reports that as `an artifact layer cannot be scanned: …`. This
/// type carries the same cause without inventing a success, and keeps a
/// constructor abort distinguishable so the fallback phase can propagate it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfferingLoadError {
    message: String,
    abort: Option<ConstructorAbort>,
    native: Option<Box<NativeOfferingFailure>>,
}

impl OfferingLoadError {
    /// Capture a load-failure message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            abort: None,
            native: None,
        }
    }

    /// A constructor abort: the Python exception the reference does not catch.
    #[must_use]
    pub fn from_abort(abort: ConstructorAbort) -> Self {
        Self {
            message: abort.to_string(),
            abort: Some(abort),
            native: None,
        }
    }

    /// A Native capability/start-up failure: never a Source exception class.
    #[must_use]
    pub fn from_native(native: NativeOfferingFailure) -> Self {
        Self {
            message: native.to_string(),
            abort: None,
            native: Some(Box::new(native)),
        }
    }

    /// The reference-shaped cause (without the `an artifact layer` prefix).
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The constructor abort, when this failure is one.
    #[must_use]
    pub fn abort(&self) -> Option<&ConstructorAbort> {
        self.abort.as_ref()
    }

    /// The typed Native cause, when this failure is one.
    ///
    /// A Native failure is an unaccepted capability gap or a start-up policy
    /// refusal; it is never an ordinary repository scan failure or Source-invalid
    /// data, and the caller must not turn it into unresolved/fallback health.
    #[must_use]
    pub fn native(&self) -> Option<&NativeOfferingFailure> {
        self.native.as_deref()
    }
}

impl std::fmt::Display for OfferingLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OfferingLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Some(native) = self.native.as_deref() {
            Some(native)
        } else {
            self.abort
                .as_ref()
                .map(|abort| abort as &dyn std::error::Error)
        }
    }
}

impl From<ConstructorAbort> for OfferingLoadError {
    fn from(abort: ConstructorAbort) -> Self {
        Self::from_abort(abort)
    }
}

impl From<NativeOfferingFailure> for OfferingLoadError {
    fn from(native: NativeOfferingFailure) -> Self {
        Self::from_native(native)
    }
}

impl From<GateFailure> for OfferingLoadError {
    fn from(failure: GateFailure) -> Self {
        match failure {
            GateFailure::Constructor(abort) => Self::from_abort(abort),
            GateFailure::Native(native) => Self::from_native(native),
        }
    }
}

/// The loaded, activation-aware offering: one declared-id mapping per activation key.
///
/// Loading is lazy per kind and memoised, exactly like the reference's
/// `CharterOfferingService` lazy properties — asking for `activated_tactics`
/// never loads (or fails on) the glossary packs.
pub struct CanonicalOfferingService {
    context: OfferingContext,
    mappings: MappingCache,
    /// The unfiltered loaded records per document kind, memoised exactly like
    /// the reference's repository property: the include accessor and the
    /// declared-id mapping then share one load.
    items: ItemCache,
    /// The agent-profile catalogue, loaded on first demand (the reference's
    /// lazy `agent_profiles` property) and reused by both the ids mapping and
    /// the include accessor. Only a completed construction is published.
    profiles: SuccessOnce<ProfileRepository>,
    /// The canonical pack-skill repository, loaded on first demand (the
    /// reference's lazy `skills` property). The Skill include/record arm and
    /// the public typed accessor share this one load. Only a completed
    /// construction is published.
    skills: SuccessOnce<crate::pack_skill_repository::PackSkillRepository>,
}

/// The memoised activation-key → declared-id mapping cache.
///
/// One entry per key, written only after that key's mapping resolved: the
/// reference's `_gated` publishes its cache after the token loop succeeded, so a
/// failure is retried rather than memoised.
type MappingCache = RefCell<BTreeMap<String, Vec<PythonString>>>;

/// The memoised document-kind → repository-record cache.
///
/// Written only after the repository construction returned, exactly like the
/// reference's lazy `self._cache[kind] = Repository(...)` property.
type ItemCache = RefCell<BTreeMap<ArtifactKind, LoadedDocumentItems>>;

/// One kind's loaded repository records plus the ids its language scope filtered out.
///
/// `scope_filtered` is the reference repository's `scope_filtered_ids`: an artifact
/// that exists on disk and passed schema validation but was excluded by its
/// `applies_to_languages` scope. It is diagnostic data, never a second catalogue.
#[derive(Clone, Default)]
pub(crate) struct LoadedDocumentItems {
    pub(crate) items: BTreeMap<PythonString, LoadedItem>,
    pub(crate) scope_filtered: BTreeSet<PythonString>,
}

impl CanonicalOfferingService {
    /// A service over `context` (roots + inferred active languages), nothing loaded yet.
    #[must_use]
    pub fn new(context: OfferingContext) -> Self {
        Self {
            context: context.into_fresh_service(),
            mappings: RefCell::new(BTreeMap::new()),
            items: RefCell::new(BTreeMap::new()),
            profiles: SuccessOnce::new(),
            skills: SuccessOnce::new(),
        }
    }

    /// The canonical pack-skill repository, loaded on first demand.
    ///
    /// The Skill arm of [`Self::document_items`] and [`Self::skill`] share this
    /// memoised load, exactly like the reference's lazy `skills` property.
    ///
    /// # Errors
    ///
    /// The repository load failure (sibling-org conflict, a fatal constructor
    /// phase), rendered as the offering service error.
    pub fn skill_repository(
        &self,
    ) -> Result<&crate::pack_skill_repository::PackSkillRepository, OfferingLoadError> {
        self.skills.get_or_try_init(|| {
            let layers = LayerDirectories::for_kind(ArtifactKind::Skill, &self.context.roots);
            let project_dir = self
                .context
                .roots
                .project
                .as_ref()
                .map(|_| layers.project.clone());
            crate::pack_skill_repository::PackSkillRepository::load(
                &layers.built_in,
                &layers.org,
                project_dir.as_deref(),
                None,
            )
            .map_err(skill_load_error)
        })
    }

    /// The effective pack-skill record the Skill arm serves (folded
    /// `overrides`/`enhances` applied), or `None` when the repository holds no
    /// such id.
    ///
    /// This is a namespaced, typed view of the canonical repository — not a
    /// second domain model. The include renderer continues to consume the
    /// canonical model-dump mapping through [`Self::artifact`].
    ///
    /// # Errors
    ///
    /// The repository load failure.
    pub fn skill(
        &self,
        skill_id: &str,
    ) -> Result<Option<&crate::pack_skill_repository::PackSkill>, OfferingLoadError> {
        Ok(self.skill_repository()?.skill(skill_id))
    }

    /// The context this service reads.
    #[must_use]
    pub fn context(&self) -> &OfferingContext {
        &self.context
    }

    /// The loaded declared ids for one activation key.
    ///
    /// # Errors
    ///
    /// [`OfferingLoadError`] when the kind's repository cannot be read (a
    /// canonical loader failure), mirroring a raising repository property.
    pub fn ids_for(&self, yaml_key: &str) -> Result<Vec<PythonString>, OfferingLoadError> {
        if let Some(cached) = self.mappings.borrow().get(yaml_key) {
            return Ok(cached.clone());
        }
        let loaded = self.load(yaml_key)?;
        self.mappings
            .borrow_mut()
            .insert(yaml_key.to_owned(), loaded.clone());
        Ok(loaded)
    }

    fn load(&self, yaml_key: &str) -> Result<Vec<PythonString>, OfferingLoadError> {
        let Some(token) = crate::effective_set::token_for(yaml_key) else {
            return Ok(Vec::new());
        };
        let Ok(kind) = ArtifactKind::from_operator_token(token) else {
            // `mission-type` is not an artifact kind and is never corpus-resolved.
            return Ok(Vec::new());
        };
        match kind {
            // The reference unions no service keys for `directive`: its scan
            // already lists every directive file by its stem. The repository is
            // still served to the include renderer (see `serves_documents`), so
            // the activation-key mapping staying empty is Source behaviour, not a
            // missing repository.
            ArtifactKind::Directive => Ok(Vec::new()),
            ArtifactKind::AgentProfile => self.load_profiles(),
            ArtifactKind::MissionStepContract
            | ArtifactKind::Tactic
            | ArtifactKind::Styleguide
            | ArtifactKind::Toolguide
            | ArtifactKind::Paradigm
            | ArtifactKind::Procedure
            | ArtifactKind::GlossaryPack => self.load_documents(kind),
            // `skill` is required-only (never resolved) and the remaining kinds
            // are not part of the activation-key universe.
            _ => Ok(Vec::new()),
        }
    }

    /// The agent-profile repository, loaded with the inferred active languages.
    ///
    /// Loading is deferred to the first demand and memoised, exactly like the
    /// reference's lazy `agent_profiles` property: a `directive` include never
    /// parses the profile catalogue.
    ///
    /// # Errors
    ///
    /// The repository load failure (the reference's raising repository
    /// property), or the unresolved built-in pack root.
    pub fn profile_repository(&self) -> Result<&ProfileRepository, OfferingLoadError> {
        self.profiles.get_or_try_init(|| {
            let roots = &self.context.roots;
            let Some(built_in) = roots.built_in.as_deref() else {
                return Err(OfferingLoadError::new(
                    "the built-in pack root is not resolved",
                ));
            };
            ProfileRepository::load_with_languages(
                built_in,
                &self.context.roots.org_roots,
                roots.project.as_deref(),
                self.context.active_languages.as_deref(),
            )
            .map_err(|error| match error {
                // The reference's repository property raises; the effective-set
                // boundary keeps the real cause typed. A canonical Source exception
                // is the existing `ConstructorAbort`; a Native start-up/capability
                // failure is the CF83 cause, never a fake Source class or a plain
                // message.
                crate::error::DoctrineError::ProfileLoadAborted { cause, .. } => match *cause {
                    crate::profile::ProfileLoadAbortCause::Source(abort) => {
                        OfferingLoadError::from_abort(abort)
                    }
                    crate::profile::ProfileLoadAbortCause::Native(native) => {
                        OfferingLoadError::from_native(native)
                    }
                },
                other => OfferingLoadError::new(other.to_string()),
            })
        })
    }

    /// The agent-profile repository, loaded with the inferred active languages.
    fn load_profiles(&self) -> Result<Vec<PythonString>, OfferingLoadError> {
        let repository = self.profile_repository()?;
        // `ActiveCharterService.agent_profiles` is one of the nine gated
        // properties, so the profile catalogue passes the same gate.
        let ids: BTreeMap<PythonString, ()> =
            repository.ids().into_iter().map(|id| (id, ())).collect();
        Ok(match self.gate() {
            Some(gate) => gate.filter(ArtifactKind::AgentProfile, &ids)?,
            None => ids.into_keys().collect(),
        })
    }

    /// The unfiltered repository records of one document kind (memoised).
    ///
    /// # Errors
    ///
    /// [`OfferingLoadError`] carrying a [`ConstructorAbort`] when a document
    /// raises the Python exception the reference's per-file `except (YAMLError,
    /// ValidationError, OSError)` does not catch: the load aborts instead of
    /// skipping (F4).
    fn document_items(&self, kind: ArtifactKind) -> Result<LoadedDocumentItems, OfferingLoadError> {
        if let Some(cached) = self.items.borrow().get(&kind) {
            return Ok(cached.clone());
        }
        let loaded = self.load_document_items(kind)?;
        self.items.borrow_mut().insert(kind, loaded.clone());
        Ok(loaded)
    }

    /// The repository keys one document kind carries, for the catalogue-miss
    /// diagnosis (`_available_catalog_ids`'s `list_all()` projection).
    ///
    /// # Errors
    ///
    /// The repository load failure.
    pub fn artifact_ids(&self, kind: ArtifactKind) -> Result<Vec<PythonString>, OfferingLoadError> {
        if !Self::serves_documents(kind) {
            return Ok(Vec::new());
        }
        Ok(self.document_items(kind)?.items.into_keys().collect())
    }

    /// The ids one document kind's language scope filtered out.
    ///
    /// # Errors
    ///
    /// The repository load failure.
    pub fn artifact_scope_filtered_ids(
        &self,
        kind: ArtifactKind,
    ) -> Result<BTreeSet<PythonString>, OfferingLoadError> {
        if !Self::serves_documents(kind) {
            return Ok(BTreeSet::new());
        }
        Ok(self.document_items(kind)?.scope_filtered)
    }

    /// The active language set the repositories filter against (`None` = unfiltered).
    #[must_use]
    pub fn active_languages(&self) -> Option<&[String]> {
        self.context.active_languages.as_deref()
    }

    /// Whether this service serves `kind` from the layered document repositories.
    ///
    /// The reference's `CharterOfferingService` exposes a repository for every
    /// artifact kind that ships standalone documents — `directives` included
    /// (`service.py`), which is what the `--include directive:<id>` renderer reads.
    /// What a directive does *not* have is an activation-key mapping; that stays
    /// with the activation scan ([`Self::ids_for`] keeps returning no directive
    /// keys), so serving the repository never widens the effective set. The three
    /// graph/resource-only kinds stay outside the set: a kind outside it has no
    /// merged record here, so callers must not mistake it for a miss.
    #[must_use]
    pub fn serves_documents(kind: ArtifactKind) -> bool {
        matches!(
            kind,
            ArtifactKind::Directive
                | ArtifactKind::Tactic
                | ArtifactKind::Styleguide
                | ArtifactKind::Toolguide
                | ArtifactKind::Paradigm
                | ArtifactKind::Procedure
                | ArtifactKind::GlossaryPack
                | ArtifactKind::MissionStepContract
                // `skill` is required-only for activation resolution (`ids_for` stays
                // empty by design), but the reference's offering service exposes a
                // `skills` repository, so an `--include skill:<id>` fetch (and the
                // `artifact:` probe) reads its layered records.
                | ArtifactKind::Skill
        )
    }

    /// The canonical merged record of `kind` under repository key `key`.
    ///
    /// This is the accessor the reference's include renderer reaches through
    /// `getattr(service, <plural>).get(lookup_id)`: the record whose overlay
    /// fields were merged onto the built-in item (and then each org layer and
    /// the project layer), not the declaring file's raw document. `Ok(None)`
    /// means the repository holds no such key, exactly like a `dict.get` miss;
    /// a kind for which this service has no repository is reported as a miss by
    /// [`Self::serves_documents`]'s contract, never as an empty record.
    ///
    /// # Errors
    ///
    /// The repository load failure, including a [`ConstructorAbort`].
    pub(crate) fn artifact(
        &self,
        kind: ArtifactKind,
        key: &str,
    ) -> Result<Option<MergedArtifact>, OfferingLoadError> {
        self.artifact_exact(kind, &PythonString::from_scalars(key))
    }

    /// [`Self::artifact`] over an exact Python string key.
    ///
    /// The layered repository keys are exact [`PythonString`]s, so an authored id
    /// containing a surrogate unit can still address its record instead of being
    /// narrowed to a Rust scalar. `Ok(None)` is the same repository miss as
    /// `dict.get`; a kind this service does not serve is also a miss (see
    /// [`Self::serves_documents`]).
    ///
    /// # Errors
    ///
    /// The repository load failure, including a [`ConstructorAbort`].
    pub(crate) fn artifact_exact(
        &self,
        kind: ArtifactKind,
        key: &PythonString,
    ) -> Result<Option<MergedArtifact>, OfferingLoadError> {
        if !Self::serves_documents(kind) {
            return Ok(None);
        }
        let loaded = self.document_items(kind)?;
        Ok(loaded.items.get(key).map(MergedArtifact::from))
    }

    /// [`Self::artifact`] behind the canonical activation gate.
    ///
    /// The reference builds the activation-aware service for exactly the
    /// `agent-profile` and `glossary-pack` fetch paths, so an artifact the
    /// allow list omits (or the effective deny holds) is a structured miss
    /// rather than a rendered body. The gate is the canonical
    /// [`ActivationGate`]; membership is decided on the repository keys, never
    /// by rewriting the requested key.
    ///
    /// # Errors
    ///
    /// The repository load failure, including a [`ConstructorAbort`].
    pub(crate) fn artifact_with_gate(
        &self,
        kind: ArtifactKind,
        key: &str,
        gate: &ActivationGate,
    ) -> Result<Option<MergedArtifact>, OfferingLoadError> {
        if !Self::serves_documents(kind) {
            return Ok(None);
        }
        let loaded = self.document_items(kind)?;
        let items = &loaded.items;
        let key = PythonString::from_scalars(key);
        if !gate
            .filter(kind, items)?
            .iter()
            .any(|admitted| admitted == &key)
        {
            return Ok(None);
        }
        Ok(items.get(&key).map(MergedArtifact::from))
    }

    /// The loaded agent profile `key`, without the activation gate.
    ///
    /// Used by the best-effort `artifact:` probe, which the reference renders on
    /// the plain (unfiltered) service.
    ///
    /// # Errors
    ///
    /// The profile repository load failure.
    pub(crate) fn profile(
        &self,
        key: &str,
    ) -> Result<Option<&crate::profile::AgentProfile>, OfferingLoadError> {
        Ok(self.profile_repository()?.get_str(key))
    }

    /// [`Self::profile`] behind the canonical activation gate.
    ///
    /// # Errors
    ///
    /// The profile repository load failure.
    pub(crate) fn profile_with_gate(
        &self,
        key: &str,
        gate: &ActivationGate,
    ) -> Result<Option<&crate::profile::AgentProfile>, OfferingLoadError> {
        let repository = self.profile_repository()?;
        let ids: BTreeMap<PythonString, ()> =
            repository.ids().into_iter().map(|id| (id, ())).collect();
        let key = PythonString::from_scalars(key);
        if !gate
            .filter(ArtifactKind::AgentProfile, &ids)?
            .iter()
            .any(|admitted| admitted == &key)
        {
            return Ok(None);
        }
        Ok(repository.get(&key))
    }

    /// One layered document repository: built-in, every org root, then project.
    ///
    /// The repository *keys* are the reference's own: a built-in item is filed
    /// under its validated model `id`; an overlay that shadows a built-in id is
    /// filed under the raw `data.get("id")` value, and a new overlay item under
    /// its model id. Only the availability/identity phases strip, so an authored
    /// `id: "  padded  "` keeps its exact repository key (F7).
    ///
    /// Admission is the canonical whole-model validation
    /// ([`crate::offering::model`]), not a hand-written top-level check, so a
    /// nested-invalid document is skipped exactly as the reference skips it (F2).
    ///
    /// The returned map is the **unfiltered** repository; [`Self::load_documents`]
    /// applies the activation gate and [`Self::artifact`] serves a record.
    ///
    /// # Errors
    ///
    /// [`OfferingLoadError`] carrying a [`ConstructorAbort`] when a document
    /// raises the Python exception the reference's per-file `except (YAMLError,
    /// ValidationError, OSError)` does not catch: the load aborts instead of
    /// skipping (F4).
    fn load_document_items(
        &self,
        kind: ArtifactKind,
    ) -> Result<LoadedDocumentItems, OfferingLoadError> {
        let mut scope_filtered: BTreeSet<PythonString> = BTreeSet::new();
        // Skill-specific arm: the canonical pack-skill repository owns its
        // whole `_load()` (per-file post-validation, sibling-org conflict and
        // `overrides`/`enhances` folding). The common layer/merge path below is
        // unchanged for every other kind (CF43 owns it).
        if kind == ArtifactKind::Skill {
            return self.load_skill_items();
        }
        let layers = LayerDirectories::for_kind(kind, &self.context.roots);
        let languages = self.context.active_languages.as_deref();
        let mut built_in: BTreeMap<PythonString, LoadedItem> = BTreeMap::new();
        for path in artifact_files(&layers.built_in, kind) {
            let Some(document) = read_document(&path)? else {
                continue;
            };
            if document.value.as_mapping().is_none() {
                continue;
            }
            // The reference's `_pre_validate` phase: after the non-mapping guard,
            // before model admission (F4's skip/fatal split).
            prevalidation::prevalidate(kind, &document, &path)?;
            let Some(id) = model::model_id(kind, &document) else {
                continue;
            };
            let scope = exact_scope(&document);
            if !admits_language(scope.as_deref(), languages) {
                scope_filtered.insert(id);
                continue;
            }
            let mapping = document.value.clone();
            let provenance = field_origins(&mapping, &document, 0);
            let markers = document.markers().clone();
            built_in.insert(
                id,
                LoadedItem {
                    document,
                    mapping,
                    scope,
                    markers,
                    provenance,
                    layer_label: Some("built-in"),
                },
            );
        }
        let mut items = built_in.clone();
        for (offset, directory) in layers
            .org
            .iter()
            .chain(std::iter::once(&layers.project))
            .enumerate()
        {
            let layer = offset + 1;
            // The reference labels every merged/inserted overlay item with the
            // layer name of the directory that produced it (`org` for each
            // configured org pack, `project` for the project layer).
            let layer_label: &'static str = if offset < layers.org.len() {
                "org"
            } else {
                "project"
            };
            for path in artifact_files(directory, kind) {
                let Some(document) = read_document(&path)? else {
                    continue;
                };
                if document.value.as_mapping().is_none() {
                    continue;
                }
                // The reference's overlay hook phase runs before `data.get("id")`
                // and before the merge, so an inline reference aborts here.
                prevalidation::prevalidate(kind, &document, &path)?;
                let Some(raw) = document.value.get("id") else {
                    continue;
                };
                if !python_truthy(raw) {
                    continue;
                }
                let raw_key = raw_repository_key(raw, &document)?;
                let shadowed = raw_key
                    .as_ref()
                    .filter(|key| built_in.contains_key(*key))
                    .cloned();
                let (key, mapping, scope, provenance, markers) = match shadowed {
                    Some(key) => {
                        let base = &built_in[&key];
                        let (mapping, provenance) = merge_mapping(base, &document, layer);
                        let mut markers = base.markers.clone();
                        markers.extend(document.markers().clone());
                        (
                            key,
                            mapping,
                            exact_scope(&document).or_else(|| base.scope.clone()),
                            provenance,
                            markers,
                        )
                    }
                    None => {
                        // A new overlay item is filed under its model id, which is
                        // the reference's `obj.id` (`_insert_overlay_item`).
                        let Some(model_id) = model::model_id(kind, &document) else {
                            continue;
                        };
                        (
                            model_id,
                            document.value.clone(),
                            exact_scope(&document),
                            field_origins(&document.value, &document, layer),
                            document.markers().clone(),
                        )
                    }
                };
                if model::admits_mapping(kind, &document, &mapping).is_none() {
                    continue;
                }
                if !admits_language(scope.as_deref(), languages) {
                    scope_filtered.insert(key);
                    continue;
                }
                scope_filtered.remove(&key);
                items.insert(
                    key,
                    LoadedItem {
                        document,
                        mapping,
                        scope,
                        markers,
                        provenance,
                        layer_label: Some(layer_label),
                    },
                );
            }
        }
        Ok(LoadedDocumentItems {
            items,
            scope_filtered,
        })
    }

    /// The layered skill records as the Source `PackSkillRepository` loads them.
    ///
    /// The repository is constructed with `namespace=None`, exactly like
    /// `CharterOfferingService.skills`; tier-specific render namespaces belong
    /// to preparation, not to repository post-validation.
    fn load_skill_items(&self) -> Result<LoadedDocumentItems, OfferingLoadError> {
        let repository = self.skill_repository()?;
        Ok(LoadedDocumentItems {
            items: repository
                .offering_records()
                .into_iter()
                .map(|record| {
                    (
                        record.key,
                        LoadedItem {
                            document: record.document,
                            mapping: record.mapping,
                            scope: None,
                            // The repository owns the **union** of every contributing
                            // layer's markers (a folded record inherits fields from a
                            // lower-tier document), so the include payload resolves
                            // them all instead of treating one layer's marker as data.
                            markers: record.markers,
                            provenance: Vec::new(),
                            layer_label: None,
                        },
                    )
                })
                .collect(),
            scope_filtered: BTreeSet::new(),
        })
    }

    /// The activation-gated declared ids of one document kind.
    ///
    /// # Errors
    ///
    /// The repository load failure (see [`Self::load_document_items`]).
    fn load_documents(&self, kind: ArtifactKind) -> Result<Vec<PythonString>, OfferingLoadError> {
        let items = self.load_document_items(kind)?.items;
        Ok(match self.gate() {
            Some(gate) => gate.filter(kind, &items)?,
            None => items.into_keys().collect(),
        })
    }

    /// The activation gate this service applies (`pack_context=None` → unfiltered).
    fn gate(&self) -> Option<&ActivationGate> {
        self.context.activation()
    }
}

impl EffectiveSetService for CanonicalOfferingService {
    fn service_keys(&self, yaml_key: &str) -> ServiceKeys {
        match self.ids_for(yaml_key) {
            Ok(ids) => ServiceKeys::Declared(ids),
            Err(error) => {
                if let Some(abort) = error.abort() {
                    ServiceKeys::Aborted(abort.clone())
                } else if let Some(native) = error.native() {
                    ServiceKeys::Native(native.clone())
                } else {
                    ServiceKeys::Failed(error.message().to_owned())
                }
            }
        }
    }
}

/// The three directories one kind reads, resolved once.
struct LayerDirectories {
    built_in: PathBuf,
    org: Vec<PathBuf>,
    project: PathBuf,
}

impl LayerDirectories {
    fn for_kind(kind: ArtifactKind, roots: &OfferingRoots) -> Self {
        let built_in_suffix = if kind == ArtifactKind::MissionStepContract {
            BUILT_IN_STEP_CONTRACTS
        } else {
            kind.plural()
        };
        let built_in = roots
            .built_in
            .as_ref()
            .map_or_else(PathBuf::new, |root| root.join(built_in_suffix));
        Self {
            built_in,
            org: roots
                .org_roots
                .iter()
                .map(|root| root.join(kind.plural()))
                .collect(),
            project: roots.project.as_ref().map_or_else(PathBuf::new, |root| {
                root.join(project_layer_dir(root, kind))
            }),
        }
    }
}

/// The project-overlay directory name for `kind` under `project_root`.
///
/// This is the reference's `CharterOfferingService._project_dir`: a project root
/// that *is* `.kittify/charter-packs` uses the per-kind project directory
/// (`PROJECT_KIND_DIRS`, singular for the runtime-managed four); the code-local
/// `src/charter/offering` fallback this crate's source facade also resolves uses
/// the kind's plural directory, exactly as the reference does.
fn project_layer_dir(project_root: &Path, kind: ArtifactKind) -> &'static str {
    let pack_root = project_root.file_name().and_then(|name| name.to_str())
        == Some("charter-packs")
        && project_root
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some(".kittify");
    if pack_root {
        kind.project_dir()
    } else {
        kind.plural()
    }
}

/// One repository record as the include renderer must see it.
///
/// `mapping` is the **merged** mapping the layered repository validated and
/// presents to the next layer (built-in fields inherited, overlay fields
/// replaced) — the reference's model-construction input.
///
/// `graph` is the same merged record as the ONE canonical object graph, built
/// from the merged mapping and the union of the contributing layers' transport
/// markers ([`exact_merged_graph`]). It is the authoritative exact view: every
/// authored code point of both inherited and overridden fields survives, and no
/// declaring file is reopened or re-parsed. `provenance` names, per merged
/// field, the layer that last wrote it.
#[derive(Clone, Debug)]
pub(crate) struct MergedArtifact {
    graph: Option<ObjectGraph>,
    provenance: Vec<FieldOrigin>,
    layer_label: Option<&'static str>,
}

impl MergedArtifact {
    /// The merged record's root as a borrowed canonical graph view.
    ///
    /// `None` is an explicit capability boundary: the merged mapping could not be
    /// canonicalised (a shape this crate never produces). A caller must not read
    /// that as "artifact missing".
    #[must_use]
    pub fn exact_root(&self) -> Option<NodeRef<'_>> {
        self.graph.as_ref().map(ObjectGraph::root_view)
    }

    /// Which merge layer supplied each merged top-level field, in merged order.
    ///
    /// This is the read-only provenance view the exact graph is composed with:
    /// the layer that last wrote each field of the record served here. It is
    /// exercised by the in-crate merge tests; no production renderer chooses a
    /// field by its origin today, so the accessor is retained for consumers
    /// (and for the provenance contract) rather than read on the serving path.
    #[must_use]
    #[allow(dead_code)]
    pub fn provenance(&self) -> &[FieldOrigin] {
        &self.provenance
    }

    /// The reference repository's `get_provenance` label for this record.
    ///
    /// `"built-in"` / `"org"` / `"project"`, or `None` when the loader holds no
    /// layer attribution for the record (a folded pack-skill record). The
    /// whole-record label is the one the renderer's provenance suffix reads; it
    /// is never derived from a per-field origin.
    #[must_use]
    pub fn provenance_label(&self) -> Option<&'static str> {
        self.layer_label
    }
}

/// Which merge layer supplied one merged top-level field.
///
/// The merge is the reference's shallow `_merge` (an overlay replaces exactly the
/// fields it declares), so a field's origin is a single layer: `0` is the
/// built-in base, then each overlay layer in load order (org roots in their
/// declared order, then the project layer). The key is spelled with the exact
/// code points of the layer that wrote it, which is also the key object CPython's
/// `dict.update` keeps for a replaced key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FieldOrigin {
    /// The merged field key, exactly as the writing layer spells it.
    pub key: PythonString,
    /// The zero-based merge layer that last wrote the field.
    pub layer: usize,
}

impl From<&LoadedItem> for MergedArtifact {
    fn from(item: &LoadedItem) -> Self {
        Self {
            graph: exact_merged_graph(&item.mapping, &item.markers),
            provenance: item.provenance.clone(),
            layer_label: item.layer_label,
        }
    }
}

/// One admitted artifact: the constructed document plus the mapping it presents
/// to the next overlay layer (raw field merge, the reference's `_merge`), the
/// union of the contributing layers' exact transport markers, and the per-field
/// merge-layer provenance of that mapping.
pub(crate) struct LoadedItem {
    document: ArtifactDocument,
    mapping: Value,
    /// The `applies_to_languages` scope, exactly, when the document declares one.
    scope: Option<Vec<PythonString>>,
    /// The union of every contributing layer's content-addressed markers.
    markers: BTreeMap<String, PythonString>,
    /// Which layer wrote each merged top-level field, in merged order.
    provenance: Vec<FieldOrigin>,
    /// The reference `get_provenance` label of the record as a whole
    /// (`"built-in"` / `"org"` / `"project"`), when the loader can attribute it.
    ///
    /// The reference sets this on every admitted insert/merge, so this is the
    /// whole-record label and not a per-field guess. `None` is an explicit
    /// capability boundary (a folded pack-skill record carries no layer table).
    layer_label: Option<&'static str>,
}

impl Clone for LoadedItem {
    fn clone(&self) -> Self {
        Self {
            document: self.document.clone(),
            mapping: self.mapping.clone(),
            scope: self.scope.clone(),
            markers: self.markers.clone(),
            provenance: self.provenance.clone(),
            layer_label: self.layer_label,
        }
    }
}

/// Convert the canonical skill repository's load failure into the offering
/// service's error, preserving a constructor abort as one.
fn skill_load_error(error: crate::pack_skill_repository::PackSkillLoadError) -> OfferingLoadError {
    match error.fatal_python() {
        Some((python, message)) => OfferingLoadError::from_abort(ConstructorAbort {
            python,
            message: message.to_owned(),
        }),
        None => OfferingLoadError::new(error.to_string()),
    }
}

/// Read one document, turning the reference's *fatal* construction failures into
/// an `Err` and its per-file skips into `Ok(None)`.
///
/// The skip/fatal split is [`crate::offering::value`]'s; this is only the
/// repository-load phase that has to tell them apart.
fn read_document(path: &Path) -> Result<Option<ArtifactDocument>, OfferingLoadError> {
    match ArtifactDocument::read(path) {
        Ok(document) => Ok(Some(document)),
        Err(error) => match error.fatal() {
            Some((python, message)) => Err(OfferingLoadError::from_abort(ConstructorAbort {
                python,
                message: message.to_owned(),
            })),
            // A skipped file is simply absent from this layer.
            None => Ok(None),
        },
    }
}

/// The repository key an overlay's raw `data.get("id")` value contributes.
///
/// The reference's `if item_id in built_in` hashes the raw value, so a list,
/// mapping, `!!set` or `!!omap` id raises `TypeError: unhashable type: …`, which
/// the loader does not catch — the whole load aborts. A `str` is the raw key;
/// every other *hashable* value (a number, a bool, `!!binary` bytes) is never
/// equal to a built-in `str` key, so it takes the insert path and its key comes
/// from the model.
fn raw_repository_key(
    raw: &Value,
    document: &ArtifactDocument,
) -> Result<Option<PythonString>, OfferingLoadError> {
    match raw {
        Value::String(text) => Ok(Some(document.exact_string(text))),
        Value::Tagged(_) => {
            if crate::profile_construct::constructed_bytes(raw).is_some() {
                return Ok(None);
            }
            Err(OfferingLoadError::from_abort(unhashable_abort(raw)))
        }
        Value::Sequence(_) => Err(OfferingLoadError::from_abort(unhashable_abort(raw))),
        Value::Mapping(_) => Err(OfferingLoadError::from_abort(unhashable_abort(raw))),
        _ => Ok(None),
    }
}

fn unhashable_abort(raw: &Value) -> ConstructorAbort {
    let python_type = match raw {
        Value::Sequence(_) => "list",
        Value::Mapping(_) => "dict",
        Value::Tagged(_) => {
            if crate::profile_construct::constructed_set(raw).is_some() {
                "set"
            } else if crate::profile_construct::constructed_omap(raw).is_some() {
                "ordereddict"
            } else if crate::profile_construct::constructed_pairs(raw).is_some() {
                "list"
            } else {
                "object"
            }
        }
        _ => "object",
    };
    ConstructorAbort {
        python: "TypeError",
        message: format!("unhashable type: '{python_type}'"),
    }
}

pub(crate) fn id_field(kind: ArtifactKind) -> &'static str {
    if kind == ArtifactKind::AgentProfile {
        "profile-id"
    } else {
        "id"
    }
}

/// The exact `applies_to_languages` list, when it is a list of strings.
fn exact_scope(document: &ArtifactDocument) -> Option<Vec<PythonString>> {
    let list = document.value.get("applies_to_languages")?.as_sequence()?;
    Some(
        list.iter()
            .filter_map(|item| item.as_str().map(|text| document.exact_string(text)))
            .collect(),
    )
}

/// The reference's `applies_to_languages_match` over the exact authored scope.
fn admits_language(scope: Option<&[PythonString]>, languages: Option<&[String]>) -> bool {
    let Some(scope) = scope else {
        // Unscoped artifacts always load.
        return true;
    };
    applies_to_languages_match_exact(scope, languages)
}

/// Python truthiness for a YAML node (the overlay's `if not item_id` guard).
fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|float| float != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Sequence(items) => !items.is_empty(),
        Value::Mapping(mapping) => !mapping.is_empty(),
        Value::Tagged(tagged) => python_truthy(&tagged.value),
    }
}

/// Field-level merge of an overlay mapping onto the built-in item it shadows.
///
/// This is the reference's `_merge` policy unchanged: a top-level field the
/// overlay declares replaces the built-in field, everything else is inherited.
/// The provenance is composed in the same pass, so the merged mapping, its exact
/// graph and the per-field origin cannot disagree.
///
/// A replaced key keeps the **base** key object, exactly like CPython's
/// `dict.update` (an equal key keeps its first spelling); the overlay supplies
/// the key text only for a field the base did not have.
fn merge_mapping(
    base: &LoadedItem,
    overlay: &ArtifactDocument,
    layer: usize,
) -> (Value, Vec<FieldOrigin>) {
    let (Some(base_mapping), Some(overlay_mapping)) =
        (base.mapping.as_mapping(), overlay.value.as_mapping())
    else {
        return (
            overlay.value.clone(),
            field_origins(&overlay.value, overlay, layer),
        );
    };
    let mut merged = base_mapping.clone();
    for (key, value) in overlay_mapping {
        merged.insert(key.clone(), value.clone());
    }
    let mut provenance = Vec::with_capacity(merged.len());
    for key in merged.keys() {
        // Provenance names the merged *fields* (model field names are strings); a
        // non-string top-level key is never a field and carries no origin here.
        let Some(text) = key.as_str() else {
            continue;
        };
        let in_base = base_mapping.contains_key(key);
        let exact = if in_base {
            base.document.exact_string(text)
        } else {
            overlay.exact_string(text)
        };
        let layer = if overlay_mapping.contains_key(key) {
            layer
        } else {
            base.provenance
                .iter()
                .find(|origin| origin.key == exact)
                .map_or(0, |origin| origin.layer)
        };
        provenance.push(FieldOrigin { key: exact, layer });
    }
    (Value::Mapping(merged), provenance)
}

/// The per-field origin of every string field of one authored mapping.
fn field_origins(mapping: &Value, document: &ArtifactDocument, layer: usize) -> Vec<FieldOrigin> {
    mapping
        .as_mapping()
        .map(|mapping| {
            mapping
                .keys()
                .filter_map(Value::as_str)
                .map(|text| FieldOrigin {
                    key: document.exact_string(text),
                    layer,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Sorted, recursive artifact files of `kind` under `directory`.
///
/// The reference walks `directory.rglob(kind.glob_pattern())`, which matches by
/// file-name suffix; a directory that does not exist contributes nothing.
fn artifact_files(directory: &Path, kind: ArtifactKind) -> Vec<PathBuf> {
    let pattern = kind.glob_pattern();
    if pattern.is_empty() || directory.as_os_str().is_empty() {
        return Vec::new();
    }
    let suffix = pattern.trim_start_matches('*');
    let mut files = Vec::new();
    collect_files(directory, suffix, &mut files);
    files.sort();
    files
}

fn collect_files(directory: &Path, suffix: &str, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_files(&path, suffix, files);
        } else if path
            .file_name()
            // Kind suffixes are ASCII. A non-scalar filename prefix must not
            // make a matching native filename disappear before its ordinary read.
            .is_some_and(|name| name.as_encoded_bytes().ends_with(suffix.as_bytes()))
        {
            files.push(path);
        }
    }
}

// ---------------------------------------------------------------------------
// Reference availability (`ActiveCharterManager.list_available_detailed`)
// ---------------------------------------------------------------------------

/// The stems the reference's availability scan offers for `kind`, exactly.
///
/// This is `list_available_detailed`'s layout, which is **not** the identity
/// index's or the offering service's: for mission step contracts only the flat
/// built-in `missions/built_in_step_contracts` directory is scanned (no org or
/// project overlay), and every other kind reads `<root>/<plural>` per layer.
/// Each discovered file contributes its config stem (the file name up to the
/// first `.`) and only when its declared name is non-empty after Python
/// `str(value).strip()`; an id-less or unparsable document is skipped.
#[must_use]
pub fn availability_stems(kind: ArtifactKind, roots: &OfferingRoots) -> Vec<String> {
    availability_entries(kind, roots)
        .into_iter()
        .map(|(stem, _)| stem)
        .collect()
}

/// The same availability scan with each file's layer retained.
///
/// Contributions are not deduplicated: the caller composes its own root-map
/// policy, and two files or layers can legitimately contribute the same stem.
/// This is not the identity index's highest-precedence winning-stem view.
#[must_use]
pub fn availability_entries(
    kind: ArtifactKind,
    roots: &OfferingRoots,
) -> Vec<(String, crate::identity::IdentityLayer)> {
    use crate::identity::IdentityLayer;

    let mut entries = Vec::new();
    let built_in = roots.built_in.as_ref().map(|root| {
        (
            IdentityLayer::BuiltIn,
            root.join(built_in_content_dir(kind)),
        )
    });
    let mut directories: Vec<_> = built_in.into_iter().collect();
    if kind != ArtifactKind::MissionStepContract {
        for root in &roots.org_roots {
            directories.push((IdentityLayer::Org, root.join(kind.plural())));
        }
        if let Some(project) = &roots.project {
            directories.push((IdentityLayer::Project, project.join(kind.project_dir())));
        }
    }
    for (layer, directory) in directories {
        for path in artifact_files(&directory, kind) {
            let Ok(document) = ArtifactDocument::read(&path) else {
                continue;
            };
            let Some(value) = document.root().lookup(id_field(kind)) else {
                continue;
            };
            if python_strip(&document.python_str(value)).is_empty() {
                continue;
            }
            let Some(stem) = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(config_stem)
            else {
                continue;
            };
            entries.push((stem, layer));
        }
    }
    entries
}

/// The directory segment a kind's built-in content ships in.
fn built_in_content_dir(kind: ArtifactKind) -> &'static str {
    if kind == ArtifactKind::MissionStepContract {
        BUILT_IN_STEP_CONTRACTS
    } else {
        kind.plural()
    }
}

/// The reference's config stem: the file name up to its first `.`.
fn config_stem(name: &str) -> String {
    name.split('.').next().unwrap_or(name).to_owned()
}

// ---------------------------------------------------------------------------
// Flat mission-type roster (the unresolved key's `fallback_ids`)
// ---------------------------------------------------------------------------

/// The ids available across every readable mission-type layer, exactly.
///
/// One contribution per root map, as the reference's `_fallback_ids` loops: the
/// base map (built-in + project) and then the base plus each org root. A map
/// whose scan raises contributes nothing; the others still count.
#[must_use]
pub fn mission_type_fallback(roots: &OfferingRoots) -> Vec<PythonString> {
    let mut ids: BTreeSet<PythonString> = BTreeSet::new();
    let built_in = roots
        .built_in
        .as_ref()
        .map(|root| root.join(BUILT_IN_MISSION_TYPES));
    let project = roots.project.as_ref().and_then(|root| {
        root.parent().map(|kittify| {
            kittify
                .join(PROJECT_MISSION_TYPES_RELATIVE_TO_KITTIFY[0])
                .join(PROJECT_MISSION_TYPES_RELATIVE_TO_KITTIFY[1])
        })
    });
    let base: Vec<PathBuf> = built_in.into_iter().chain(project).collect();
    if let Some(found) = scan_map(&base) {
        ids.extend(found);
    }
    for org_root in &roots.org_roots {
        let mut directories = base.clone();
        directories.push(org_root.join(ORG_MISSION_TYPES_SUBDIR));
        if let Some(found) = scan_map(&directories) {
            ids.extend(found);
        }
    }
    ids.into_iter().collect()
}

/// Scan every directory of one root map; `None` when any of them raises.
fn scan_map(directories: &[PathBuf]) -> Option<Vec<PythonString>> {
    let mut ids = Vec::new();
    for directory in directories {
        match scan_mission_types_dir(directory) {
            Ok(found) => ids.extend(found),
            Err(_) => return None,
        }
    }
    Some(ids)
}

/// The schema-validating, loud-fail, flat `mission_types` scan (`scan_mission_types_dir`).
///
/// A directory that does not exist contributes nothing; a directory that exists
/// but cannot be read, or a file that is malformed or fails schema validation,
/// raises. That is deliberately stricter than a `glob`, which silently swallows
/// both — the reference makes the same distinction, and the mission-type
/// fallback degrades per root map rather than per file.
pub fn scan_mission_types_dir(directory: &Path) -> Result<Vec<PythonString>, OfferingLoadError> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(directory).map_err(|error| {
        OfferingLoadError::new(format!(
            "mission-type directory exists but cannot be read: {}: {error}",
            directory.display()
        ))
    })?;
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".yaml"))
        })
        .collect();
    files.sort();
    files.iter().map(|path| load_mission_type(path)).collect()
}

/// Load one mission-type file, enforcing id-equals-stem like the reference.
///
/// `scan_mission_types_dir` loud-fails on every malformed/unreadable/unadmitted
/// file (the reference's `_load_layered_mission_type_file`), and the model
/// validation itself is the shared [`crate::offering::model`] authority — so the
/// roster and the artifact repositories cannot disagree about a document.
fn load_mission_type(path: &Path) -> Result<PythonString, OfferingLoadError> {
    let invalid = || OfferingLoadError::new(format!("invalid mission type {}", path.display()));
    let Some(document) = read_document(path)? else {
        return Err(invalid());
    };
    if document.value.as_mapping().is_none() {
        return Err(invalid());
    }
    let Some(id) = model::admits_mission_type(&document) else {
        return Err(invalid());
    };
    let Ok(text) = id.to_scalar_string() else {
        return Err(invalid());
    };
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if text != stem {
        return Err(OfferingLoadError::new(format!(
            "MissionType id {text:?} in {} does not match filename stem {stem:?}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
        )));
    }
    Ok(id)
}

/// CF43 unit coverage for the merged-record provenance composition.
///
/// The integration proof (Source child vs Native child over real packs) lives in
/// `tests/cf43_exact_include_delivery.rs`; these are the private-building-block
/// assertions that fixture project trees cannot reach directly.
#[cfg(test)]
mod cf43_merged_provenance_tests {
    use super::*;

    fn document(text: &str) -> ArtifactDocument {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("doc.yaml");
        std::fs::write(&path, text).expect("write");
        ArtifactDocument::read(&path).expect("read")
    }

    fn loaded(document: ArtifactDocument) -> LoadedItem {
        let mapping = document.value.clone();
        let markers = document.markers().clone();
        let provenance = field_origins(&mapping, &document, 0);
        LoadedItem {
            document,
            mapping,
            scope: None,
            markers,
            provenance,
            layer_label: Some("built-in"),
        }
    }

    #[test]
    fn shallow_merge_keeps_inherited_exact_fields_and_records_the_writing_layer() {
        // The built-in layer authors the inherited fields. The overlay declares one
        // replacement carrying a lone surrogate (reachable only through the marker
        // transport) and one new field, so both merge directions are exercised.
        let base = loaded(document(
            "id: fixture\nprovenance: built-in\nname: Base name\n",
        ));
        let overlay = document("name: \"overlay \\ud800 name\"\nnotes: added by overlay\n");
        let (mapping, provenance) = merge_mapping(&base, &overlay, 2);

        // Base field order survives; a replaced key keeps its position, a new key
        // is appended (CPython `dict.update`).
        assert_eq!(
            mapping
                .as_mapping()
                .expect("mapping")
                .keys()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>(),
            vec!["id", "provenance", "name", "notes"]
        );
        // Every merged field names the layer that last wrote it.
        let layers: Vec<(String, usize)> = provenance
            .iter()
            .map(|origin| {
                (
                    origin.key.to_scalar_string().expect("scalar field name"),
                    origin.layer,
                )
            })
            .collect();
        assert_eq!(
            layers,
            vec![
                ("id".to_owned(), 0),
                ("provenance".to_owned(), 0),
                ("name".to_owned(), 2),
                ("notes".to_owned(), 2),
            ]
        );

        // The unioned content-addressed markers are what make the merged mapping
        // exact again: the inherited field is the base text and the replaced field
        // keeps the overlay's lone surrogate instead of its marker spelling.
        let mut markers = base.markers.clone();
        markers.extend(overlay.markers().clone());
        let graph = exact_merged_graph(&mapping, &markers).expect("merged graph");
        let root = graph.root_view();
        assert_eq!(
            root.lookup("provenance")
                .expect("inherited field")
                .as_str()
                .expect("string"),
            &PythonString::from_scalars("built-in")
        );
        let replaced = root
            .lookup("name")
            .expect("replaced field")
            .as_str()
            .expect("string")
            .clone();
        assert!(
            replaced.codepoints().contains(&0xD800),
            "the exact code point must survive the merge: {:?}",
            replaced.codepoints()
        );
    }

    #[test]
    fn the_merged_artifact_exposes_the_exact_graph_and_the_field_origins() {
        let base = loaded(document(
            "id: fixture\nprovenance: built-in\nname: Base name\n",
        ));
        let overlay = document("name: \"overlay \\ud800 name\"\n");
        let (mapping, provenance) = merge_mapping(&base, &overlay, 1);
        let mut markers = base.markers.clone();
        markers.extend(overlay.markers().clone());
        let artifact = MergedArtifact::from(&LoadedItem {
            document: overlay,
            mapping,
            scope: None,
            markers,
            provenance: provenance.clone(),
            layer_label: Some("org"),
        });

        // `None` here would be a capability boundary, never "artifact missing".
        let root = artifact.exact_root().expect("merged exact root");
        assert!(
            root.lookup("name")
                .expect("name")
                .as_str()
                .expect("string")
                .codepoints()
                .contains(&0xD800)
        );
        assert_eq!(artifact.provenance(), provenance.as_slice());
    }
}
