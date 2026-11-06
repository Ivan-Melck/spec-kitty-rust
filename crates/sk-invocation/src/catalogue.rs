//! Source-compatible profile catalogue for invocation dispatch.
//!
//! Mirrors `src/specify_cli/invocation/registry.py::ProfileRegistry`. The
//! reference class is a *thin wrapper* over two independently-gated sources, and
//! this module keeps exactly that shape:
//!
//! * the **doctrine** catalogue is the built-in + org-pack + project
//!   charter-pack layers loaded through [`sk_doctrine::ProfileRepository`], with
//!   the `applies_to_languages` filter bound to the project's inferred active
//!   languages and the `activated_agent_profiles` / `deactivated_agent_profiles`
//!   three-state gate (org required floor netted out) applied by
//!   [`sk_doctrine::ActivationState`];
//! * the **legacy** `.kittify/profiles` layer is a separate repository loaded by
//!   the *same* loader, ungated and language-unfiltered, and it **wins** id
//!   collisions. It is outside the doctrine activation model, so no activation
//!   state is consulted for it.
//!
//! Two catalogues are exposed because the reference exposes two:
//! [`ProfileRegistry::resolve`] reads the dispatch *routing* catalogue and
//! [`ProfileRegistry::resolve_local`] reads the operator-facing local catalogue.
//! They coincide today (every doctrine layer is routing-admitted in #4114) and
//! stay separate APIs so a future provenance filter cannot silently change
//! `--profile` resolution.
//!
//! [`ProfileRegistry::resolve`] is a catalogue lookup, not lineage composition:
//! lineage (`specializes_from`) is `AgentProfileRepository::resolve`, reachable
//! through [`ProfileRegistry::doctrine_repository`] /
//! [`ProfileRegistry::resolve_with_lineage`].
//!
//! Identity is exact: ids are [`sk_kernel::PythonString`], never a lossy
//! `String` projection, so a Source-valid profile whose id holds a lone
//! surrogate is catalogued, resolvable and routable rather than dropped.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sk_doctrine::{
    AgentProfile, CatalogueRoots, DoctrineError, DrgGraph, InferRequest, LanguageScopeService,
    ProfileRepository, SkippedProfile, catalogue_roots, infer_repo_languages, profile_id_label,
    project_activation_state,
};
use sk_kernel::PythonString;

use crate::router::RoutableProfile;

/// The default routing priority the reference model applies when a profile does
/// not author one.
const DEFAULT_ROUTING_PRIORITY: i64 = sk_doctrine::DEFAULT_ROUTING_PRIORITY;

/// The catalogue's error type.
#[derive(Debug)]
pub enum CatalogueError {
    /// A doctrine read (roots, ledger, floor, activation index) failed.
    Doctrine(DoctrineError),
    /// The id is in no layer of the catalogue being resolved.
    ProfileNotFound {
        /// The requested id, rendered exactly (scalar text or escaped `repr`).
        profile: String,
        /// The catalogue's ids, rendered the same way, sorted.
        available: Vec<String>,
    },
}

impl std::fmt::Display for CatalogueError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Doctrine(error) => write!(formatter, "{error}"),
            Self::ProfileNotFound { profile, available } => write!(
                formatter,
                "Profile '{profile}' not found. Available: {available:?}"
            ),
        }
    }
}

impl std::error::Error for CatalogueError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Doctrine(error) => Some(error),
            Self::ProfileNotFound { .. } => None,
        }
    }
}

impl From<DoctrineError> for CatalogueError {
    fn from(error: DoctrineError) -> Self {
        Self::Doctrine(error)
    }
}

/// The inputs a catalogue was built from.
///
/// Explicit so a caller can reuse resolved roots (and so tests can build a
/// catalogue over their own trees) without re-deriving the pack chain.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CatalogueOptions {
    /// The repository root the activation ledger, org charter and language
    /// inference read from.
    pub repo_root: PathBuf,
    /// The built-in / org / project / legacy pack roots.
    pub roots: CatalogueRoots,
    /// The project's active languages; `None` disables the scoped-profile filter.
    pub active_languages: Option<Vec<String>>,
}

impl CatalogueOptions {
    /// Resolve the canonical options for `repo_root`.
    ///
    /// Roots come from the one lenient chain
    /// ([`sk_doctrine::catalogue_roots`]); the active languages come from the
    /// canonical language-scope service (`configured → compiled → interview`,
    /// with the no-signal / explicit-empty distinction preserved).
    pub fn resolve(repo_root: &Path) -> Result<Self, CatalogueError> {
        let roots = catalogue_roots(repo_root)?;
        let active_languages =
            infer_repo_languages(&InferRequest::new(Some(repo_root)))?.into_option();
        Ok(Self {
            repo_root: repo_root.to_path_buf(),
            roots,
            active_languages,
        })
    }

    /// The same inference through an explicit service instance (tests).
    pub fn with_language_service(
        repo_root: &Path,
        roots: CatalogueRoots,
        service: &mut LanguageScopeService,
    ) -> Result<Self, CatalogueError> {
        let active_languages = service
            .infer(&InferRequest::new(Some(repo_root)))?
            .into_option();
        Ok(Self {
            repo_root: repo_root.to_path_buf(),
            roots,
            active_languages,
        })
    }
}

/// The Source-compatible profile catalogue.
#[derive(Clone, Debug)]
pub struct ProfileRegistry {
    routing: BTreeMap<PythonString, AgentProfile>,
    local: BTreeMap<PythonString, AgentProfile>,
    provenance: BTreeMap<PythonString, &'static str>,
    source_paths: BTreeMap<PythonString, PathBuf>,
    legacy_ids: BTreeSet<PythonString>,
    skipped: Vec<SkippedProfile>,
    scope_filtered_ids: BTreeSet<String>,
    scope_filtered_exact_ids: BTreeSet<PythonString>,
    activation_configured: bool,
    options: CatalogueOptions,
    doctrine: ProfileRepository,
    legacy: Option<ProfileRepository>,
}

impl ProfileRegistry {
    /// Build the catalogue for a repository root.
    pub fn load(repo_root: &Path) -> Result<Self, CatalogueError> {
        Self::load_with(CatalogueOptions::resolve(repo_root)?)
    }

    /// Build the catalogue over explicit options.
    pub fn load_with(options: CatalogueOptions) -> Result<Self, CatalogueError> {
        let roots = &options.roots;
        // Doctrine layers: activation-gated and language-filtered.
        let doctrine = ProfileRepository::load_with_languages(
            &roots.built_in,
            &roots.org_roots,
            roots.project.as_deref(),
            options.active_languages.as_deref(),
        )?;
        // The legacy invocation overlay: same loader, no org packs, no language
        // filter, no activation gate (the reference constructs a second
        // repository with only `project_dir` pointing at `.kittify/profiles`).
        let legacy = match &roots.legacy_project {
            Some(directory) => Some(ProfileRepository::load_profile_directories(
                &roots.built_in.join("agent_profiles"),
                &[],
                Some(directory),
                None,
            )?),
            None => None,
        };
        let state = project_activation_state(
            &options.repo_root,
            &roots.built_in,
            &roots.org_roots,
            roots.project.as_deref(),
        )?;
        let activation_configured = state.agent_profiles_allow_configured();

        let mut routing: BTreeMap<PythonString, AgentProfile> = BTreeMap::new();
        let mut local: BTreeMap<PythonString, AgentProfile> = BTreeMap::new();
        let mut provenance = BTreeMap::new();
        let mut source_paths = BTreeMap::new();
        for id in doctrine.ids() {
            if !state.admits_profile_exact(&id) {
                continue;
            }
            let Some(profile) = doctrine.get(&id) else {
                continue;
            };
            local.insert(id.clone(), profile.clone());
            // Every doctrine layer is routing-admitted (#4114); the provenance
            // filter is applied explicitly so a future non-admitted layer cannot
            // leak into the routing catalogue.
            if matches!(
                doctrine.provenance(&id),
                Some("built-in" | "org" | "project")
            ) {
                routing.insert(id.clone(), profile.clone());
            }
            provenance.insert(id.clone(), doctrine.provenance(&id).unwrap_or("built-in"));
            source_paths.insert(id, profile.path.clone());
        }
        let mut legacy_ids = BTreeSet::new();
        if let Some(legacy) = &legacy {
            for profile in legacy
                .ids()
                .into_iter()
                .filter_map(|id| legacy.get(&id).map(|profile| (id, profile)))
                .filter(|(id, _)| legacy.provenance(id) == Some("project"))
            {
                let (id, profile) = profile;
                // Ungated and language-unfiltered: the legacy overlay wins.
                routing.insert(id.clone(), profile.clone());
                local.insert(id.clone(), profile.clone());
                provenance.insert(id.clone(), "project");
                source_paths.insert(id.clone(), profile.path.clone());
                legacy_ids.insert(id);
            }
        }

        let mut skipped: Vec<SkippedProfile> = doctrine.skipped().to_vec();
        let mut scope_filtered_ids = doctrine.scope_filtered_ids().clone();
        let mut scope_filtered_exact_ids = doctrine.scope_filtered_exact_ids().clone();
        if let Some(legacy) = &legacy {
            skipped.extend(legacy.skipped().iter().cloned());
        }
        // A legacy overlay re-admits an id the doctrine scope filter dropped.
        for id in &legacy_ids {
            scope_filtered_exact_ids.remove(id);
            scope_filtered_ids.remove(&profile_id_label(id));
        }
        skipped.sort_by(|left, right| left.path.cmp(&right.path));

        Ok(Self {
            routing,
            local,
            provenance,
            source_paths,
            legacy_ids,
            skipped,
            scope_filtered_ids,
            scope_filtered_exact_ids,
            activation_configured,
            options,
            doctrine,
            legacy,
        })
    }

    /// The routing catalogue's ids, sorted by exact code points.
    pub fn ids(&self) -> Vec<PythonString> {
        self.routing.keys().cloned().collect()
    }

    /// A routing-catalogue profile by exact id.
    pub fn get(&self, id: &PythonString) -> Option<&AgentProfile> {
        self.routing.get(id)
    }

    /// A routing-catalogue profile by scalar id.
    pub fn get_str(&self, id: &str) -> Option<&AgentProfile> {
        self.get(&PythonString::from_scalars(id))
    }

    /// Resolve an id in the dispatch routing catalogue.
    pub fn resolve(&self, id: &PythonString) -> Result<&AgentProfile, CatalogueError> {
        self.routing
            .get(id)
            .ok_or_else(|| CatalogueError::ProfileNotFound {
                profile: profile_id_label(id),
                available: self.ids().iter().map(profile_id_label).collect(),
            })
    }

    /// [`Self::resolve`] for a scalar id.
    pub fn resolve_str(&self, id: &str) -> Result<&AgentProfile, CatalogueError> {
        self.resolve(&PythonString::from_scalars(id))
    }

    /// Resolve an id in the operator-facing local catalogue.
    ///
    /// The local catalogue keeps the same activation and language gates and the
    /// same legacy overlay, and admits every doctrine layer (no routing
    /// provenance filter). It backs `--profile <id>` resolution.
    pub fn resolve_local(&self, id: &PythonString) -> Result<&AgentProfile, CatalogueError> {
        self.local
            .get(id)
            .ok_or_else(|| CatalogueError::ProfileNotFound {
                profile: profile_id_label(id),
                available: self
                    .local
                    .keys()
                    .map(profile_id_label)
                    .collect::<Vec<String>>(),
            })
    }

    /// [`Self::resolve_local`] for a scalar id.
    pub fn resolve_local_str(&self, id: &str) -> Result<&AgentProfile, CatalogueError> {
        self.resolve_local(&PythonString::from_scalars(id))
    }

    /// Every routing-catalogue profile, in sorted exact-id order.
    ///
    /// This is the **exact** handoff for a consumer that needs the rich model
    /// (dispatch, StepContract, governance): ids are
    /// [`sk_kernel::PythonString`], and every accessor on [`AgentProfile`]
    /// (`name`, `roles`, `preferred_model`, `effort`, `domain_keywords`,
    /// `canonical_verbs`, ...) returns authored code points. A consumer whose
    /// own projection uses Rust `String` must adapt those fields to
    /// `PythonString` rather than dropping or substituting a profile: a
    /// Source-valid non-scalar identity has no `String` projection at all.
    pub fn profiles(&self) -> Vec<&AgentProfile> {
        self.routing.values().collect()
    }

    /// The rich model for one routing id (`None` when it is not catalogued).
    ///
    /// Same lookup as [`Self::get`]; the name is the one a dispatch consumer
    /// reads. Exact, never projected.
    pub fn profile(&self, id: &PythonString) -> Option<&AgentProfile> {
        self.get(id)
    }

    /// The router view of one routing id, if it is catalogued.
    pub fn routing_profile(&self, id: &PythonString) -> Option<RoutableProfile> {
        let profile = self.get(id)?;
        let role = profile.role()?;
        Some(RoutableProfile::new(
            profile.id.clone(),
            role,
            i32::try_from(profile.routing_priority()).unwrap_or(DEFAULT_ROUTING_PRIORITY as i32),
            profile.domain_keywords(),
            profile.canonical_verbs(),
        ))
    }

    /// Routing ids that have no scalar `String` projection at all.
    ///
    /// A consumer that must return a `String` id (an old adapter, a JSON field)
    /// uses this to report a real capability gap for these ids instead of
    /// dropping the profile or inventing a placeholder id. It is empty for every
    /// shipped catalogue.
    pub fn non_scalar_ids(&self) -> Vec<PythonString> {
        self.routing
            .keys()
            .filter(|id| id.to_scalar_string().is_err())
            .cloned()
            .collect()
    }

    /// The router's caller-supplied snapshot of the routing catalogue.
    ///
    /// Profiles are emitted in sorted exact-id order, exactly like
    /// `list_all()`, and every field is authored data: no default role, model or
    /// keyword is invented. `routing_priority` keeps the reference model's
    /// default of 50 when the profile does not author one.
    pub fn routing_profiles(&self) -> Vec<RoutableProfile> {
        self.routing
            .keys()
            .filter_map(|id| self.routing_profile(id))
            .collect()
    }

    /// Whether the routing catalogue holds any profile.
    pub fn has_profiles(&self) -> bool {
        !self.routing.is_empty()
    }

    /// The provenance layer label of a routing id.
    pub fn provenance(&self, id: &PythonString) -> Option<&'static str> {
        self.provenance.get(id).copied()
    }

    /// The file a routing id was loaded from.
    pub fn source_path(&self, id: &PythonString) -> Option<&Path> {
        self.source_paths.get(id).map(PathBuf::as_path)
    }

    /// Every refused profile file, from the doctrine and legacy loads.
    pub fn skipped(&self) -> &[SkippedProfile] {
        &self.skipped
    }

    /// Doctrine ids dropped by the active language scope (scalar projection).
    pub fn scope_filtered_ids(&self) -> &BTreeSet<String> {
        &self.scope_filtered_ids
    }

    /// Doctrine ids dropped by the active language scope, exactly.
    pub fn scope_filtered_exact_ids(&self) -> &BTreeSet<PythonString> {
        &self.scope_filtered_exact_ids
    }

    /// Whether an `activated_agent_profiles` allow set is configured.
    ///
    /// `false` is the three-state *absent* case (admit every profile); an
    /// explicit `activated_agent_profiles: []` is configured-and-empty, so the
    /// doctrine catalogue is genuinely empty rather than falling back.
    pub fn activation_configured(&self) -> bool {
        self.activation_configured
    }

    /// The repository root this catalogue reads activation and language from.
    pub fn repo_root(&self) -> &Path {
        &self.options.repo_root
    }

    /// The inputs this catalogue was built from.
    pub fn options(&self) -> &CatalogueOptions {
        &self.options
    }

    /// The project's inferred active languages.
    pub fn active_languages(&self) -> Option<&[String]> {
        self.options.active_languages.as_deref()
    }

    /// Ids supplied by the legacy `.kittify/profiles` overlay.
    pub fn legacy_ids(&self) -> &BTreeSet<PythonString> {
        &self.legacy_ids
    }

    /// The uncomposed doctrine repository (lineage, skips, provenance).
    pub fn doctrine_repository(&self) -> &ProfileRepository {
        &self.doctrine
    }

    /// The uncomposed legacy repository, when `.kittify/profiles` exists.
    pub fn legacy_repository(&self) -> Option<&ProfileRepository> {
        self.legacy.as_ref()
    }

    /// Resolve `id` with its `specializes_from` lineage composed.
    ///
    /// Looks the id up in the routing catalogue first, then in the *uncomposed*
    /// doctrine/legacy repositories (an id the activation gate dropped is not
    /// lineage-resolvable through the catalogue). The graph is the caller's, so
    /// no second DRG is loaded or cached here.
    pub fn resolve_with_lineage(
        &self,
        id: &PythonString,
        graph: &DrgGraph,
    ) -> Result<AgentProfile, CatalogueError> {
        if self.routing.contains_key(id) {
            return Ok(self.doctrine.resolve(id, graph)?);
        }
        if let Some(legacy) = &self.legacy
            && legacy.get(id).is_some()
        {
            return Ok(legacy.resolve(id, graph)?);
        }
        if self.doctrine.get(id).is_some() {
            return Ok(self.doctrine.resolve(id, graph)?);
        }
        Err(CatalogueError::ProfileNotFound {
            profile: profile_id_label(id),
            available: self.ids().iter().map(profile_id_label).collect(),
        })
    }
}
