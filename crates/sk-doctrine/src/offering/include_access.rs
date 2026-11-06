//! The lazily constructed authorities one non-template `--include` selector reads.
//!
//! The reference's `build_charter_context_include`
//! (`src/charter/activation/context.py`) builds exactly what the selector needs and
//! nothing else:
//!
//! 1. parse the selector; `section:` and `template:` return before any doctrine work;
//! 2. resolve the effective deny that annotates a deactivated selector
//!    (`_denied_kind` → `load_denied` → `resolve_deny`/`EffectiveDeny`);
//! 3. build **one** `IdentityIndex` (lazy, per-kind scans) that resolves the
//!    declared id/stem (`IdentityIndex.lookup_key`);
//! 4. build the offering service — `_build_offering_service` for every kind except
//!    `agent-profile`/`glossary-pack`, which use the activation-aware
//!    `_build_active_charter_service` — with the project root from
//!    `resolve_project_root` and the languages from `infer_repo_languages`;
//! 5. render the querior kind's repository record (`repo.get(lookup_id)`), which
//!    loads only that kind's layered files.
//!
//! This module owns steps 3–4's *construction* (plus the deny decision of step 2) so
//! the CLI compatibility layer only wires a root and the CLI error composition, and so
//! a `directive` include never parses the profile catalogue, the DRG bundle or an
//! unrelated kind.
//!
//! # One authority per decision
//!
//! * Pack roots: `sk_charter::PackRoots::resolve_existing_source` (the lenient
//!   existing chain `resolve_pack_chain(..., strict=False)` reads) plus the
//!   `sk_charter::resolve_source_project_layer` candidate list — never a locally
//!   spelled candidate order.
//! * Languages: [`OfferingContext::infer_languages`], the same projection
//!   `_build_offering_service` makes.
//! * Identity: [`IdentityIndex::from_roots`] over the built-in layer, the existing
//!   org chain and the project **pack** root (`IdentityIndex.build` uses
//!   `project_pack_root`, not the offering project candidate; this asymmetry is the
//!   reference's).
//! * Deny: [`crate::reference_catalog::denied_by_kind`] — the crate's ONE deny
//!   composition (ledger deny lists → identity spellings → org-required floor),
//!   with no identity index built when the ledger holds no deny list (NFR-003).
//!   The `catalog.references` admission filter reads the same composition, so
//!   there is no second copy of it anywhere.
//! * Type gate access: [`ActivationGate::from_project`], the canonical
//!   `PackContext.from_config` + `load_denied` composition.

use std::cell::OnceCell;
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;

use crate::effective_set::{OfferingContext, OfferingRoots};
use crate::identity::IdentityIndex;
use crate::offering::{ActivationGate, CanonicalOfferingService};

/// The project pack directory the reference's identity index uses.
const PROJECT_PACK_DIR: &str = ".kittify/charter-packs";

/// Why the include authorities could not be constructed.
///
/// `config` distinguishes the reference's `ActiveCharterConfigError` family (an
/// activation-ledger / org-required-floor / pack-config read while deciding a deny
/// annotation) from a plain construction failure, because the CLI emits the former
/// with its Source machine code and the latter without one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncludeAccessError {
    message: String,
    config: bool,
}

impl IncludeAccessError {
    /// A construction failure (no Source `ActiveCharterConfigError`).
    #[must_use]
    pub fn setup(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            config: false,
        }
    }

    /// An activation-config/floor/ledger failure (`ActiveCharterConfigError`).
    #[must_use]
    pub fn config(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            config: true,
        }
    }

    /// The rendered reason.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Whether this is the reference's `ActiveCharterConfigError` family.
    #[must_use]
    pub fn is_config(&self) -> bool {
        self.config
    }
}

impl std::fmt::Display for IncludeAccessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for IncludeAccessError {}

/// The lazily built authorities one `--include` invocation may read.
pub struct IncludeAccess {
    repo_root: PathBuf,
    roots: OfferingRoots,
    identity: IdentityIndex,
    context: OnceCell<OfferingContext>,
    offering: OnceCell<CanonicalOfferingService>,
    gate: OnceCell<Result<ActivationGate, IncludeAccessError>>,
}

impl IncludeAccess {
    /// Use already-resolved pack roots (test fixtures and embedders).
    ///
    /// The identity index is built over the supplied layer set — `roots.project` is the
    /// project layer — because an explicit layer set is the caller's whole world. The
    /// canonical [`Self::load`] path keeps the reference's own asymmetry instead: its
    /// identity index reads the project **pack** root while its offering service reads
    /// the first existing offering-project candidate.
    ///
    /// # Errors
    ///
    /// [`IncludeAccessError::setup`] when `roots.built_in` is not resolved: the
    /// reference's `built_in_dir` raises in that state, so the include cannot
    /// silently render an empty layer.
    pub fn with_roots(repo_root: &Path, roots: OfferingRoots) -> Result<Self, IncludeAccessError> {
        let project = roots.project.clone();
        Self::from_parts(repo_root, roots, project.as_deref())
    }

    /// Resolve the roots and identity for `repo_root`; nothing is loaded yet.
    ///
    /// The language inference and the activation gate are deferred to
    /// [`Self::offering`] / [`Self::gate`] because the reference reaches them only
    /// after a selector's deny annotation and only for the kinds that need them.
    ///
    /// # Errors
    ///
    /// The pack-root resolution failure (the reference's raising service builder),
    /// including an unresolvable built-in layer.
    pub fn load(repo_root: &Path) -> Result<Self, IncludeAccessError> {
        let roots = source_roots(repo_root)?;
        // The reference's identity index uses `project_pack_root(repo_root)`, not the
        // offering's `resolve_project_root` candidate list.
        let project_pack = repo_root.join(PROJECT_PACK_DIR);
        let identity_project = project_pack.is_dir().then_some(project_pack.as_path());
        Self::from_parts(repo_root, roots, identity_project)
    }

    /// Assemble the access authority from resolved parts.
    fn from_parts(
        repo_root: &Path,
        roots: OfferingRoots,
        identity_project: Option<&Path>,
    ) -> Result<Self, IncludeAccessError> {
        let Some(built_in) = roots.built_in.as_deref() else {
            return Err(IncludeAccessError::setup(
                "no pack root could be resolved for built-in",
            ));
        };
        let identity = IdentityIndex::from_roots(built_in, &roots.org_roots, identity_project);
        Ok(Self {
            repo_root: repo_root.to_path_buf(),
            roots,
            identity,
            context: OnceCell::new(),
            offering: OnceCell::new(),
            gate: OnceCell::new(),
        })
    }

    /// The identity index the selector's declared id/stem resolves through.
    #[must_use]
    pub fn identity(&self) -> &IdentityIndex {
        &self.identity
    }

    /// The offering context (roots + the reference's inferred active languages).
    ///
    /// # Errors
    ///
    /// The `infer_repo_languages` failure, which the reference's
    /// `_build_offering_service` propagates.
    pub fn context(&self) -> Result<&OfferingContext, IncludeAccessError> {
        if self.context.get().is_none() {
            let languages = OfferingContext::infer_languages(&self.repo_root)
                .map_err(IncludeAccessError::setup)?;
            let _ = self.context.set(OfferingContext::new(
                self.repo_root.clone(),
                self.roots.clone(),
                languages,
            ));
        }
        Ok(self.context.get().expect("the context was just built"))
    }

    /// The plain (unfiltered) canonical offering service.
    ///
    /// This is the reference's `_build_offering_service` half: raw by design, with
    /// no activation filter, and lazy per kind.
    ///
    /// # Errors
    ///
    /// The language-inference failure of [`Self::context`].
    pub fn offering(&self) -> Result<&CanonicalOfferingService, IncludeAccessError> {
        let context = self.context()?.clone();
        if self.offering.get().is_none() {
            let _ = self.offering.set(CanonicalOfferingService::new(context));
        }
        Ok(self
            .offering
            .get()
            .expect("the offering service was just built"))
    }

    /// The canonical activation gate (`PackContext.from_config` + `_gated`).
    ///
    /// Built on demand, because the reference wraps the offering service for
    /// exactly the `agent-profile` and `glossary-pack` include paths.
    ///
    /// # Errors
    ///
    /// [`IncludeAccessError::config`] for the activation-ledger / org-floor /
    /// config read failure (`ActiveCharterConfigError`), matching the reference's
    /// exception family.
    pub fn gate(&self) -> Result<&ActivationGate, IncludeAccessError> {
        if self.gate.get().is_none() {
            let built_in = self.roots.built_in.as_deref().ok_or_else(|| {
                IncludeAccessError::config("no pack root could be resolved for built-in")
            })?;
            let gate = ActivationGate::from_project(
                &self.repo_root,
                built_in,
                &self.roots.org_roots,
                self.roots.project.as_deref(),
            )
            .map_err(IncludeAccessError::config);
            let _ = self.gate.set(gate);
        }
        match self.gate.get().expect("the gate was just built") {
            Ok(gate) => Ok(gate),
            Err(error) => Err(error.clone()),
        }
    }

    /// The kind whose effective deny holds `identifier`, if any.
    ///
    /// This is the reference's `_denied_kind`: kinds with a non-empty deny list (all
    /// activatable kinds, or only `kinds` when given) are netted against the
    /// org-required floor, sorted by plural, and the selector's identity spellings
    /// are intersected with the surviving deny spellings. A hit annotates the
    /// selector; it never refuses it.
    ///
    /// # Errors
    ///
    /// [`IncludeAccessError::config`] when the ledger, the org-required floor or
    /// the deny resolution fails — the reference's `ActiveCharterConfigError`
    /// family, raised while the annotation is decided.
    pub fn denied_kind(
        &self,
        identifier: &str,
        kinds: Option<&[ArtifactKind]>,
    ) -> Result<Option<ArtifactKind>, IncludeAccessError> {
        // One deny composition for the whole crate: `crate::reference_catalog`
        // also answers the `catalog.references` admission question with it.
        let denied = crate::reference_catalog::denied_by_kind(&self.repo_root, kinds)
            .map_err(|error| IncludeAccessError::config(error.to_string()))?;
        Ok(crate::reference_catalog::denied_kind_of(
            identifier,
            &denied,
            &self.identity,
        ))
    }
}

/// The pack roots one include reads, resolved through the canonical pack authority.
///
/// * built-in: the packaged/explicitly-overridden built-in layer;
/// * org: `PackRoots::resolve_existing_source` — the lenient existing chain, in
///   declaration order (a not-yet-fetched pack is dropped, an unreadable registry
///   contributes nothing, an unexpandable `local_path` still fails);
/// * project: `sk_charter::resolve_source_project_layer` — `.kittify/charter-packs`
///   when present, else the code-local `src/charter/offering` layer, exactly the
///   reference's `resolve_project_root`.
///
/// # Errors
///
/// A path-expansion failure (an unset environment variable in a declared pack).
pub fn source_roots(repo_root: &Path) -> Result<OfferingRoots, IncludeAccessError> {
    let built_in = crate::org_charter::built_in_search(repo_root)
        .find_root()
        .ok();
    let config = sk_charter::load_project_config(repo_root, &sk_charter::Limits::default()).ok();
    let expansion = sk_charter::PathExpansion::from_env();
    let search = crate::org_charter::built_in_search(repo_root);
    let resolved = sk_charter::PackRoots::resolve_existing_source(
        repo_root,
        config.as_ref(),
        &expansion,
        Some(&search),
    )
    .map_err(|error| IncludeAccessError::setup(error.to_string()))?;
    Ok(OfferingRoots {
        built_in,
        org_roots: resolved
            .organizations
            .iter()
            .map(|pack| pack.root.clone())
            .collect(),
        project: sk_charter::resolve_source_project_layer(repo_root),
    })
}
